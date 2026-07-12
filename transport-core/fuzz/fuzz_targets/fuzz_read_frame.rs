#![no_main]
use libfuzzer_sys::fuzz_target;
use transport_core::protocol::FrameCodec;

/// Fuzz target: read_frame с произвольным input.
/// Цель: найти panic/crash при malformed TLS-record frames.
/// Ожидаемое поведение: всегда возвращает Err, никогда не паникует.
fuzz_target!(|data: &[u8]| {
    let key = [0u8; 32];
    let prefix = [0u8; 8];
    let codec = FrameCodec::new(&key, prefix, prefix, 0);
    let mut cursor = std::io::Cursor::new(data);

    // Используем block_on т.к. read_frame async.
    // cargo-fuzz не поддерживает async напрямую — используем tokio runtime.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _ = rt.block_on(codec.read_frame(&mut cursor));
});
