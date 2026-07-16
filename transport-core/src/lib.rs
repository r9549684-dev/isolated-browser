// transport-core/src/lib.rs

pub mod auth;
pub mod endpoint;
pub mod error;
pub mod failover;
pub mod protocol;
pub mod proxy;
pub mod rate_limiter;
pub mod replay;
pub mod steal;
pub mod tcp_handler;
pub mod tls;

use std::ffi::{CStr, CString};
use std::net::SocketAddr;
use std::os::raw::{c_char, c_int, c_ushort};
use std::panic;
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use endpoint::EndpointConfig;
use failover::FailoverManager;

pub use endpoint::{decode_hex32, parse_endpoints, EndpointConfigError};
pub use failover::{Clock, SystemClock, FAILOVER_MIN_INTERVAL, FAILURE_THRESHOLD};

/// Максимальное время ожидания подтверждения bind() от рабочего потока перед тем,
/// как transport_start() сдаётся и возвращает ошибку. Bind — синхронная быстрая
/// операция ОС; 5с — большой запас (обычно <10ms), защищает от зависания FFI-вызова
/// навечно, если что-то пошло совсем не так в рантайме.
const BIND_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);

/// Результат попытки bind, отправляемый рабочим потоком обратно в transport_start().
enum BindResult {
    Bound,
    Failed(String),
}

/// Общее состояние запущенного транспорта: слот для transport_get_status() +
/// хендл для реального shutdown в transport_stop() (устраняет TODO B1 из черновика).
struct RunningTransport {
    failover: Arc<FailoverManager>,
    /// Сигнализирует accept-loop завершиться. Реальный shutdown вместо no-op.
    shutdown: Arc<tokio::sync::Notify>,
    join_handle: std::thread::JoinHandle<()>,
}

static ACTIVE: OnceLock<Mutex<Option<RunningTransport>>> = OnceLock::new();

fn active_slot() -> &'static Mutex<Option<RunningTransport>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Общая обёртка: выполняет `f`, ловит панику через catch_unwind (паника через
/// extern "C" границу — UB, реализация Ревизора A4 критично #2), возвращает
/// `on_panic` при срабатывании паники.
fn guarded<F, R>(f: F, on_panic: R) -> R
where
    F: FnOnce() -> R + panic::UnwindSafe,
{
    match panic::catch_unwind(f) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("transport: panic caught at FFI boundary, returning fail-closed value");
            on_panic
        }
    }
}

/// FFI-обёртка для запуска транспорта из Flutter через dart:ffi.
///
/// Запускает SOCKS5-прокси с multi-endpoint failover в отдельном потоке+рантайме.
/// Возвращает 0 ТОЛЬКО после того, как TcpListener::bind() реально выполнен успешно
/// (fail-closed: неопределённость перед подтверждением бинда = не стартуем).
/// Возвращает -1 при любой ошибке, включая: невалидный JSON, порт занят, паника.
///
/// Если транспорт уже запущен (предыдущий успешный start без stop) — возвращает -1,
/// чтобы предотвратить утечку потоков/рантаймов на повторные вызовы (Ревизор A4 п.
/// «Повторный transport_start течёт потоками»).
///
/// # Safety
/// `endpoints_json` должна быть валидной NUL-terminated C-строкой.
/// `key_bytes` должен указывать на буфер >= 32 байт (может быть null — тогда игнорируется,
/// см. примечание про зарезервированный параметр ниже).
#[no_mangle]
pub unsafe extern "C" fn transport_start(
    socks_port: c_ushort,
    endpoints_json: *const c_char,
    key_bytes: *const u8,
    rate_limit_bps: u64,
) -> c_int {
    let endpoints_json_addr = endpoints_json as usize;
    let key_bytes_addr = key_bytes as usize;

    guarded(
        move || {
            let endpoints_json = endpoints_json_addr as *const c_char;
            let key_bytes = key_bytes_addr as *const u8;

            if endpoints_json.is_null() || key_bytes.is_null() {
                return -1;
            }

            // Защита от повторного старта без stop() (Ревизор A4: "течёт потоками").
            {
                let guard = active_slot().lock().unwrap();
                if guard.is_some() {
                    eprintln!("transport_start: already running, call transport_stop() first");
                    return -1;
                }
            }

            let json_str = match CStr::from_ptr(endpoints_json).to_str() {
                Ok(s) => s,
                Err(_) => return -1,
            };

            let endpoints: Vec<EndpointConfig> = match endpoint::parse_endpoints(json_str) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("transport_start: invalid endpoints_json: {}", e);
                    return -1;
                }
            };

            // key_bytes: параметр ЗАРЕЗЕРВИРОВАН (см. Socks5Proxy в A3 — key нигде не
            // участвует в шифровании; канал строится на server_pub/psk/session-key).
            // Копируем ради сохранения frozen G-Contract подписи, но не используем
            // криптографически. Явно задокументировано, чтобы не вводить в заблуждение.
            let mut key = [0u8; 32];
            std::ptr::copy_nonoverlapping(key_bytes, key.as_mut_ptr(), 32);

            let bind_addr: SocketAddr = match format!("127.0.0.1:{}", socks_port).parse() {
                Ok(a) => a,
                Err(_) => return -1,
            };
            if socks_port == 0 {
                eprintln!("transport_start: socks_port == 0 rejected (would bind ephemeral, not what caller expects)");
                return -1;
            }

            let failover = Arc::new(FailoverManager::new(endpoints));
            let shutdown = Arc::new(tokio::sync::Notify::new());

            // Канал подтверждения bind — устраняет критичный баг Ревизора A4 #1:
            // раньше transport_start возвращал 0 ДО реального TcpListener::bind().
            let (tx, rx) = sync_channel::<BindResult>(1);

            let failover_for_thread = failover.clone();
            let shutdown_for_thread = shutdown.clone();
            let rate_limit_bps_captured = rate_limit_bps;

            let join_handle = std::thread::spawn(move || {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(BindResult::Failed(format!("tokio runtime init failed: {}", e)));
                        return;
                    }
                };

                rt.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind(bind_addr).await {
                        Ok(l) => l,
                        Err(e) => {
                            let _ = tx.send(BindResult::Failed(format!("bind({}) failed: {}", bind_addr, e)));
                            return;
                        }
                    };

                    // Bind подтверждён — сигнализируем вызывающей стороне ДО начала accept-loop.
                    let _ = tx.send(BindResult::Bound);

                    let mut proxy = proxy::Socks5Proxy::new(bind_addr, failover_for_thread, key);
                    if rate_limit_bps_captured > 0 {
                        proxy = proxy.with_rate_limit(rate_limit_bps_captured);
                    }

                    tokio::select! {
                        res = proxy.run_with_listener(listener) => {
                            if let Err(e) = res {
                                eprintln!("transport: proxy loop exited with error: {}", e);
                            }
                        }
                        _ = shutdown_for_thread.notified() => {
                            eprintln!("transport: shutdown signal received, stopping accept loop");
                        }
                    }
                });
            });

            match rx.recv_timeout(BIND_CONFIRM_TIMEOUT) {
                Ok(BindResult::Bound) => {
                    let mut guard = active_slot().lock().unwrap();
                    *guard = Some(RunningTransport { failover, shutdown, join_handle });
                    0
                }
                Ok(BindResult::Failed(msg)) => {
                    eprintln!("transport_start: {}", msg);
                    let _ = join_handle.join();
                    -1
                }
                Err(RecvTimeoutError::Timeout) => {
                    eprintln!("transport_start: bind confirmation timed out after {:?}", BIND_CONFIRM_TIMEOUT);
                    -1
                }
                Err(RecvTimeoutError::Disconnected) => {
                    eprintln!("transport_start: worker thread died before confirming bind");
                    let _ = join_handle.join();
                    -1
                }
            }
        },
        -1,
    )
}

/// Возвращает текущий статус транспорта как JSON C-строку (через serde_json —
/// устраняет критичный баг Ревизора A4 #3: ручная сборка JSON без экранирования
/// host допускала невалидный JSON при host, содержащем `"` или `\`).
///
/// Формат при running: {"running":true,"current_endpoint":{"host":"...","port":N},
///                       "failures":N,"last_switch_unix_ms":N}
/// Формат при остановленном транспорте: {"running":false}
///
/// Вызывающий обязан освободить память через transport_free_string().
/// Возвращает null при панике (не должно происходить в норме).
#[no_mangle]
pub extern "C" fn transport_get_status() -> *const c_char {
    guarded(
        || {
            let guard = active_slot().lock().unwrap();
            let json = match guard.as_ref() {
                Some(rt) => {
                    let ep = rt.failover.current_endpoint();
                    serde_json::json!({
                        "running": true,
                        "current_endpoint": { "host": ep.host, "port": ep.port },
                        "failures": rt.failover.failure_count(),
                        "last_switch_unix_ms": rt.failover.last_switch_unix_ms(),
                    })
                    .to_string()
                }
                None => serde_json::json!({ "running": false }).to_string(),
            };

            match CString::new(json) {
                Ok(c) => c.into_raw(),
                Err(_) => std::ptr::null(),
            }
        },
        std::ptr::null(),
    )
}

/// Освобождает строку, возвращённую transport_get_status().
/// No-op на null (безопасно вызывать после неудачного get_status()).
///
/// # Safety
/// `ptr` должен быть получен ИМЕННО из transport_get_status() и освобождён ровно один раз.
#[no_mangle]
pub unsafe extern "C" fn transport_free_string(ptr: *mut c_char) {
    let addr = ptr as usize;
    guarded(
        move || {
            let ptr = addr as *mut c_char;
            if !ptr.is_null() {
                drop(CString::from_raw(ptr));
            }
        },
        (),
    );
}

/// Возвращает версию транспортного модуля как статическую C-строку (НЕ требует free —
/// иной контракт владения, чем transport_get_status(); задокументировано явно, чтобы
/// не спутать с heap-строкой, transport_free_string на неё вызывать нельзя).
#[no_mangle]
pub extern "C" fn transport_version() -> *const c_char {
    static VERSION: &[u8] = b"0.2.0\0";
    VERSION.as_ptr() as *const c_char
}

/// Останавливает транспортный модуль: сигнализирует accept-loop завершиться,
/// дожидается завершения рабочего потока и сбрасывает статус-слот.
/// Устраняет критичный баг Ревизора A4: раньше transport_stop был no-op,
/// из-за чего get_status() вечно отдавал running:true, а повторный start()
/// не мог забиндиться на тот же порт.
///
/// Возвращает 0 при успехе (включая случай "транспорт не был запущен" — идемпотентно),
/// -1 при панике во время остановки.
#[no_mangle]
pub extern "C" fn transport_stop() -> c_int {
    guarded(
        || {
            let running = {
                let mut guard = active_slot().lock().unwrap();
                guard.take()
            };

            match running {
                Some(rt) => {
                    rt.shutdown.notify_one();
                    // Дожидаемся реального освобождения порта перед возвратом,
                    // иначе следующий transport_start() может застать порт занятым.
                    if rt.join_handle.join().is_err() {
                        eprintln!("transport_stop: worker thread panicked during shutdown");
                    }
                    0
                }
                None => 0, // уже остановлен — идемпотентно, не ошибка
            }
        },
        -1,
    )
}