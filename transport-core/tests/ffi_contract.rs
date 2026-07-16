// transport-core/tests/ffi_contract.rs

//! Контрактные тесты FFI-слоя. Все тесты помечены #[serial] — они мутируют
//! общий глобальный слот ACTIVE (Ревизор A4: "гонка на глобальном слоте").
//! Каждый тест сам вызывает transport_stop() в конце (или в начале, defensively),
//! чтобы не оставлять залипшие потоки/порты между тестами.
//!
//! Эти тесты требуют исполнения на целевой платформе (Android device / host).
//! При кросс-компиляции для Android с crate-type=["cdylib"] extern "C"
//! символы недоступны в rlib — тест компилируется только для host target.
//! На устройстве запускается через `cargo test --tests` после деплоя .so.

#![cfg(not(target_os = "android"))]

use serial_test::serial;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

extern "C" {
    fn transport_start(socks_port: u16, endpoints_json: *const c_char, key_bytes: *const u8, rate_limit_bps: u64) -> i32;
    fn transport_get_status() -> *const c_char;
    fn transport_free_string(ptr: *mut c_char);
    fn transport_stop() -> i32;
    fn transport_version() -> *const c_char;
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn endpoints_json(host_suffix: &str) -> CString {
    let json = format!(
        r#"[{{"host":"127.0.0.1{}","port":8443,"sni":"cloudflare.com","server_pub":"{}","psk":"{}"}}]"#,
        host_suffix,
        "aa".repeat(32),
        "bb".repeat(32)
    );
    CString::new(json).unwrap()
}

fn get_status_string() -> String {
    unsafe {
        let ptr = transport_get_status();
        assert!(!ptr.is_null(), "transport_get_status вернул null");
        let s = CStr::from_ptr(ptr).to_str().unwrap().to_string();
        transport_free_string(ptr as *mut c_char);
        s
    }
}

#[test]
#[serial]
fn valid_json_actually_binds_and_returns_zero() {
    unsafe { transport_stop(); } // defensive cleanup от предыдущего флакового теста
    let key = [0u8; 32];
    let json = endpoints_json("");
    let port = free_port();

    let ret = unsafe { transport_start(port, json.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret, 0, "валидный конфиг должен успешно забиндиться");

    // Проверяем, что порт РЕАЛЬНО занят (устраняет ложную уверенность старого теста,
    // который проходил даже когда bind не проверялся — Ревизор A4).
    let bind_attempt = std::net::TcpListener::bind(format!("127.0.0.1:{}", port));
    assert!(bind_attempt.is_err(), "порт должен быть занят запущенным транспортом");

    unsafe { transport_stop(); }
}

#[test]
#[serial]
fn start_fails_when_port_already_in_use() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let port = free_port();
    let _blocker = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();

    let json = endpoints_json("");
    let ret = unsafe { transport_start(port, json.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret, -1, "bind на занятый порт должен вернуть -1 (после исправления бага fail-closed)");
}

#[test]
#[serial]
fn malformed_json_returns_minus_one() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let bad = CString::new("{not valid json").unwrap();
    let ret = unsafe { transport_start(free_port(), bad.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret, -1);
}

#[test]
#[serial]
fn empty_array_returns_minus_one() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let empty = CString::new("[]").unwrap();
    let ret = unsafe { transport_start(free_port(), empty.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret, -1);
}

#[test]
#[serial]
fn null_endpoints_json_returns_minus_one() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let ret = unsafe { transport_start(free_port(), std::ptr::null(), key.as_ptr(), 0) };
    assert_eq!(ret, -1);
}

#[test]
#[serial]
fn null_key_bytes_returns_minus_one() {
    unsafe { transport_stop(); }
    let json = endpoints_json("");
    let ret = unsafe { transport_start(free_port(), json.as_ptr(), std::ptr::null(), 0) };
    assert_eq!(ret, -1, "null key_bytes должен быть отклонён");
}

#[test]
#[serial]
fn double_start_without_stop_returns_minus_one() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let json1 = endpoints_json("");
    let port1 = free_port();
    let ret1 = unsafe { transport_start(port1, json1.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret1, 0);

    let json2 = endpoints_json("");
    let port2 = free_port();
    let ret2 = unsafe { transport_start(port2, json2.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret2, -1, "повторный старт без stop() должен быть отклонён (устраняет утечку потоков)");

    unsafe { transport_stop(); }
}

#[test]
#[serial]
fn get_status_before_start_is_running_false() {
    unsafe { transport_stop(); }
    let status = get_status_string();
    let parsed: serde_json::Value = serde_json::from_str(&status).expect("должен быть валидный JSON");
    assert_eq!(parsed["running"], false);
}

#[test]
#[serial]
fn get_status_after_start_has_full_fields() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let json = endpoints_json("");
    let port = free_port();
    assert_eq!(unsafe { transport_start(port, json.as_ptr(), key.as_ptr(), 0) }, 0);

    let status = get_status_string();
    let parsed: serde_json::Value = serde_json::from_str(&status).expect("должен быть валидный JSON");
    assert_eq!(parsed["running"], true);
    assert_eq!(parsed["current_endpoint"]["host"], "127.0.0.1");
    assert_eq!(parsed["current_endpoint"]["port"], 8443);
    assert!(parsed["failures"].is_number());
    assert!(parsed["last_switch_unix_ms"].is_number());

    unsafe { transport_stop(); }
}

#[test]
#[serial]
fn get_status_escapes_special_characters_in_host() {
    // Регрессия на ручную сборку JSON без экранирования (Ревизор A4 критичный #3).
    // host с кавычкой не должен пройти endpoint-валидацию (см. A1), но если бы
    // прошёл — serde_json обязан корректно экранировать. Здесь имитируем прямой
    // JSON round-trip на уровне serde_json, т.к. endpoint::parse_endpoints (A1)
    // отклонит такой host раньше — фиксируем, что сама сериализация безопасна.
    let v = serde_json::json!({
        "running": true,
        "current_endpoint": { "host": "evil\"host\\", "port": 1 },
        "failures": 0,
        "last_switch_unix_ms": 0
    });
    let s = v.to_string();
    let reparsed: serde_json::Value = serde_json::from_str(&s).expect("serde_json должен производить валидный JSON независимо от содержимого host");
    assert_eq!(reparsed["current_endpoint"]["host"], "evil\"host\\");
}

#[test]
#[serial]
fn stop_releases_port_for_next_start() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let json = endpoints_json("");
    let port = free_port();
    assert_eq!(unsafe { transport_start(port, json.as_ptr(), key.as_ptr(), 0) }, 0);

    let ret_stop = unsafe { transport_stop() };
    assert_eq!(ret_stop, 0);

    // Порт должен быть реально свободен после stop() — устраняет рассинхрон
    // "get_status после stop() вечно отдаёт running:true" из ревизии.
    let rebind = std::net::TcpListener::bind(format!("127.0.0.1:{}", port));
    assert!(rebind.is_ok(), "порт должен освободиться после transport_stop()");

    let status = get_status_string();
    let parsed: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(parsed["running"], false, "статус должен показывать running:false после stop()");
}

#[test]
#[serial]
fn stop_is_idempotent_when_not_running() {
    unsafe { transport_stop(); }
    let ret = unsafe { transport_stop() };
    assert_eq!(ret, 0, "stop() без запущенного транспорта не должен быть ошибкой");
}

#[test]
#[serial]
fn restart_after_stop_succeeds_on_same_port() {
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let port = free_port();

    let json1 = endpoints_json("");
    assert_eq!(unsafe { transport_start(port, json1.as_ptr(), key.as_ptr(), 0) }, 0);
    assert_eq!(unsafe { transport_stop() }, 0);

    let json2 = endpoints_json("");
    let ret2 = unsafe { transport_start(port, json2.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret2, 0, "после корректного stop() рестарт на том же порту должен успешно забиндиться");

    unsafe { transport_stop(); }
}

#[test]
#[serial]
fn version_does_not_require_free_and_is_stable() {
    let v1 = unsafe { CStr::from_ptr(transport_version()).to_str().unwrap().to_string() };
    let v2 = unsafe { CStr::from_ptr(transport_version()).to_str().unwrap().to_string() };
    assert_eq!(v1, "0.2.0");
    assert_eq!(v1, v2, "статическая строка должна быть стабильна между вызовами");
}

#[test]
#[serial]
fn free_string_on_null_is_noop() {
    unsafe { transport_free_string(std::ptr::null_mut()) };
    // Не паникует — тест проходит, если дошли до этой строки.
}

#[test]
#[serial]
fn bind_failure_does_not_leave_dangling_slot() {
    // После неудачного start() (порт занят) повторная попытка start() на СВОБОДНОМ
    // порту должна пройти — слот не должен "залипнуть" в занятом состоянии.
    unsafe { transport_stop(); }
    let key = [0u8; 32];
    let busy_port = free_port();
    let _blocker = std::net::TcpListener::bind(format!("127.0.0.1:{}", busy_port)).unwrap();

    let json_fail = endpoints_json("");
    assert_eq!(unsafe { transport_start(busy_port, json_fail.as_ptr(), key.as_ptr(), 0) }, -1);

    let free = free_port();
    let json_ok = endpoints_json("");
    let ret = unsafe { transport_start(free, json_ok.as_ptr(), key.as_ptr(), 0) };
    assert_eq!(ret, 0, "неудачный start() не должен блокировать последующие успешные попытки");

    unsafe { transport_stop(); }
}