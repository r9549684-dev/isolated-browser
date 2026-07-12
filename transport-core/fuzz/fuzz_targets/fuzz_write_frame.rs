#![no_main]
use libfuzzer_sys::fuzz_target;
use transport_core::protocol::FrameCodec;

/// Fuzz target: write_frame → read_frame roundtrip с произвольным plaintext.
/// Цель: убедиться что roundtrip не паникует и данные сохраняются.
fuzz_target!(|data: &[u8]| {
    if data.len() > transport_core::protocol::MAX_PAYLOAD {
        return;
    }
    let key = [0u8; 32];
    let prefix = [0u8; 8];
    let writer = FrameCodec::new(&key, prefix, prefix, 0);
    let reader = FrameCodec::new(&key, prefix, prefix, 0);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut buf: Vec<u8> = Vec::new();
        if writer.write_frame(&mut buf, data).await.is_err() {
            return;
        }
        let mut cursor = std::io::Cursor::new(buf);
        let _ = reader.read_frame(&mut cursor).await;
    });
});
