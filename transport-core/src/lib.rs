pub mod auth;
pub mod error;
pub mod protocol;
pub mod proxy;
pub mod rate_limiter;
pub mod replay;
pub mod steal;
pub mod tcp_handler;
pub mod tls;

use std::ffi::CStr;
use std::net::SocketAddr;
use std::os::raw::{c_char, c_int, c_ushort};

/// FFI-обёртка для вызова из Flutter через dart:ffi.
///
/// Запускает SOCKS5-прокси в отдельном tokio-рантайме.
/// Возвращает 0 при успехе, -1 при ошибке.
///
/// Параметры:
///   socks_port     — локальный порт прокси (например 18080)
///   gateway_host   — C-строка с хостом gateway (например "gw.example.com")
///   gateway_port   — порт gateway (например 443)
///   key_bytes      — указатель на 32-байтовый ключ шифрования
///   sni_list       — C-строка с SNI-доменами через запятую (например "cloudflare.com,google.com")
///   server_pub     — указатель на 32-байтовый X25519 public key сервера
///   rate_limit_bps — ограничение скорости в байтах/сек (0 = без ограничений)
///
/// # Safety
/// gateway_host и sni_list должны быть валидными C-string.
/// key_bytes и server_pub должны указывать на буфер >= 32 байт.
#[no_mangle]
pub unsafe extern "C" fn transport_start(
    socks_port: c_ushort,
    gateway_host: *const c_char,
    gateway_port: c_ushort,
    key_bytes: *const u8,
    sni_list: *const c_char,
    server_pub: *const u8,
    rate_limit_bps: u64,
) -> c_int {
    if gateway_host.is_null() || key_bytes.is_null() || sni_list.is_null() || server_pub.is_null() {
        return -1;
    }

    let host = match CStr::from_ptr(gateway_host).to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => return -1,
    };

    let sni_str = match CStr::from_ptr(sni_list).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let sni_pool: Vec<String> = sni_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if sni_pool.is_empty() {
        return -1;
    }

    let mut key = [0u8; 32];
    std::ptr::copy_nonoverlapping(key_bytes, key.as_mut_ptr(), 32);

    let mut server_public = [0u8; 32];
    std::ptr::copy_nonoverlapping(server_pub, server_public.as_mut_ptr(), 32);

    let bind: SocketAddr = match format!("127.0.0.1:{}", socks_port).parse() {
        Ok(a) => a,
        Err(_) => return -1,
    };

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let mut proxy = proxy::Socks5Proxy::new(bind, host, gateway_port, key, sni_pool, server_public);
        
        if rate_limit_bps > 0 {
            proxy = proxy.with_rate_limit(rate_limit_bps);
        }
        
        if let Err(e) = rt.block_on(proxy.run()) {
            eprintln!("transport error: {}", e);
        }
    });

    0
}

/// Возвращает версию транспортного модуля как C-строку.
/// Указывает на статическую память — вызывающий НЕ освобождает.
#[no_mangle]
pub extern "C" fn transport_version() -> *const c_char {
    static VERSION: &[u8] = b"0.1.0\0";
    VERSION.as_ptr() as *const c_char
}

/// Останавливает транспортный модуль.
/// Возвращает 0 при успехе, -1 при ошибке.
#[no_mangle]
pub extern "C" fn transport_stop() -> c_int {
    // TODO: Реализовать корректное завершение spawned thread
    // Пока возвращаем 0 для удовлетворения FFI-контракта.
    0
}
