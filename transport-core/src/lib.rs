pub mod error;
pub mod protocol;
pub mod proxy;
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
///   socks_port    — локальный порт прокси (например 18080)
///   gateway_host  — C-строка с хостом gateway (например "gw.example.com")
///   gateway_port  — порт gateway (например 443)
///   key_bytes     — указатель на 32-байтовый ключ
///
/// # Safety
/// gateway_host должен быть валидным C-string.
/// key_bytes должен указывать на буфер >= 32 байт.
#[no_mangle]
pub unsafe extern "C" fn transport_start(
    socks_port: c_ushort,
    gateway_host: *const c_char,
    gateway_port: c_ushort,
    key_bytes: *const u8,
) -> c_int {
    if gateway_host.is_null() || key_bytes.is_null() {
        return -1;
    }

    let host = match CStr::from_ptr(gateway_host).to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => return -1,
    };

    let mut key = [0u8; 32];
    std::ptr::copy_nonoverlapping(key_bytes, key.as_mut_ptr(), 32);

    let bind: SocketAddr = match format!("127.0.0.1:{}", socks_port).parse() {
        Ok(a) => a,
        Err(_) => return -1,
    };

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let proxy = proxy::Socks5Proxy::new(bind, host, gateway_port, key);
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
