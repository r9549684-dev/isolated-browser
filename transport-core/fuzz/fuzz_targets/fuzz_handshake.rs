#![no_main]
use libfuzzer_sys::fuzz_target;
use transport_core::steal::{client_handshake, server_derive_session, read_auth_frame};

/// Fuzz target: ECDHE handshake с произвольным auth frame input.
/// Цель: найти panic при malformed auth frames.
/// Ожидаемое поведение: возвращает Err, никогда не паникует.
fuzz_target!(|data: &[u8]| {
    if data.len() < 80 {
        return;
    }
    let server_secret = [0x42u8; 32];
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut stream = std::io::Cursor::new(data);
        // read_auth_frame должен не паниковать на любых данных
        if let Ok((ephemeral, token, _c2s, _s2c)) = read_auth_frame(&mut stream).await {
            // server_derive_session должен не паниковать
            let _ = server_derive_session(&server_secret, &ephemeral, &token);
        }
    });
});
