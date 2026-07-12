/// Протокол с маскировкой под TLS 1.3 Application Data.
///
/// Формат фрейма:
///
///  ┌──────────┬────────────┬──────────┬─────┬──────────┬─────────────────────┐
///  │  0x17    │  0x03 0x03 │  length  │ kid │ counter  │    ciphertext       │
///  │  1 byte  │  2 bytes   │  2 bytes  │ 1B  │  4 bytes  │    N + 16 bytes     │
///  └──────────┴────────────┴──────────┴─────┴──────────┴─────────────────────┘
///
/// 0x17 = TLS Application Data content type
/// 0x03 0x03 = TLS 1.2 version (для совместимости с DPI)
/// length = KID_SIZE + COUNTER_SIZE + ciphertext.len() (big-endian u16)
/// kid = key identifier (для ротации ключей)
/// counter = монотонный u32 counter (big-endian)
/// ciphertext = ChaCha20-Poly1305 encrypt(plaintext)
///
/// Nonce = send_prefix(8) || counter(4) = 12 bytes
/// Session prefix — случайные 8 байт, генерируются per-session.
/// Разные prefix для client→server и server→client (предотвращает nonce collision).
///
/// Rekey trigger: при counter >= REKEY_THRESHOLD возвращается TransportError::RekeyNeeded.
/// Жёсткий лимит: 2^32 фреймов на сессию, после чего — reconnect.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use bytes::{BufMut, BytesMut};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::RngCore;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::TransportError;
use crate::replay::ReplayWindow;

pub const TLS_CONTENT_TYPE: u8 = 0x17;
pub const TLS_VERSION_MAJOR: u8 = 0x03;
pub const TLS_VERSION_MINOR: u8 = 0x03;
pub const TLS_RECORD_HEADER_SIZE: usize = 5;
pub const KID_SIZE: usize = 1;
pub const COUNTER_SIZE: usize = 4;
pub const TAG_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 12;
pub const PREFIX_SIZE: usize = 8;

pub const MAX_PAYLOAD: usize = 16 * 1024;
pub const MAX_FRAME_BODY: usize = KID_SIZE + COUNTER_SIZE + MAX_PAYLOAD + TAG_SIZE;

/// Rekey threshold: ~4 миллиарда фреймов (оставляем margin до 2^32).
pub const REKEY_THRESHOLD: u64 = 0xF000_0000;
/// Жёсткий лимит: 2^32 фреймов, после которого counter переполняется u32.
pub const REKEY_HARD_LIMIT: u64 = 0x1_0000_0000;

/// Размер sliding window для replay protection (в фреймах).
pub const REPLAY_WINDOW_SIZE: u64 = 64;

pub struct FrameCodec {
    cipher: ChaCha20Poly1305,
    send_prefix: [u8; PREFIX_SIZE],
    recv_prefix: [u8; PREFIX_SIZE],
    send_counter: AtomicU64,
    recv_replay: Mutex<ReplayWindow>,
    kid: u8,
}

impl FrameCodec {
    pub fn new(
        key: &[u8; 32],
        send_prefix: [u8; PREFIX_SIZE],
        recv_prefix: [u8; PREFIX_SIZE],
        kid: u8,
    ) -> Self {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
        Self {
            cipher,
            send_prefix,
            recv_prefix,
            send_counter: AtomicU64::new(0),
            recv_replay: Mutex::new(ReplayWindow::new(REPLAY_WINDOW_SIZE)),
            kid,
        }
    }

    pub fn generate_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    }

    pub fn generate_prefix() -> [u8; PREFIX_SIZE] {
        let mut prefix = [0u8; PREFIX_SIZE];
        OsRng.fill_bytes(&mut prefix);
        prefix
    }

    pub fn kid(&self) -> u8 {
        self.kid
    }

    pub fn send_counter(&self) -> u64 {
        self.send_counter.load(Ordering::SeqCst)
    }

    /// Кодирует plaintext в бинарный фрейм и пишет в writer.
    /// Использует монотонный counter как часть nonce.
    /// Возвращает RekeyNeeded при достижении threshold.
    pub async fn write_frame<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        plaintext: &[u8],
    ) -> Result<(), TransportError> {
        if plaintext.len() > MAX_PAYLOAD {
            return Err(TransportError::FrameTooLarge(plaintext.len(), MAX_PAYLOAD));
        }

        let counter = self.send_counter.fetch_add(1, Ordering::SeqCst);
        if counter >= REKEY_THRESHOLD {
            return Err(TransportError::RekeyNeeded(counter, REKEY_THRESHOLD));
        }

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        nonce_bytes[..PREFIX_SIZE].copy_from_slice(&self.send_prefix);
        nonce_bytes[PREFIX_SIZE..].copy_from_slice(&(counter as u32).to_be_bytes());
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| TransportError::Crypto(e.to_string()))?;

        let body_len = (KID_SIZE + COUNTER_SIZE + ciphertext.len()) as u16;

        let mut buf =
            BytesMut::with_capacity(TLS_RECORD_HEADER_SIZE + KID_SIZE + COUNTER_SIZE + ciphertext.len());
        buf.put_u8(TLS_CONTENT_TYPE);
        buf.put_u8(TLS_VERSION_MAJOR);
        buf.put_u8(TLS_VERSION_MINOR);
        buf.put_u16(body_len);
        buf.put_u8(self.kid);
        buf.put_u32(counter as u32);
        buf.put_slice(&ciphertext);

        writer.write_all(&buf).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Читает один фрейм из reader, проверяет replay, расшифровывает.
    pub async fn read_frame<R: AsyncRead + Unpin>(
        &self,
        reader: &mut R,
    ) -> Result<Vec<u8>, TransportError> {
        let mut header = [0u8; TLS_RECORD_HEADER_SIZE];
        reader.read_exact(&mut header).await?;

        let content_type = header[0];
        if content_type != TLS_CONTENT_TYPE {
            return Err(TransportError::Protocol(format!(
                "bad content type: 0x{:02X} (expected 0x{:02X})",
                content_type, TLS_CONTENT_TYPE
            )));
        }

        let version_major = header[1];
        let version_minor = header[2];
        if version_major != TLS_VERSION_MAJOR || version_minor != TLS_VERSION_MINOR {
            return Err(TransportError::Protocol(format!(
                "bad TLS version: {}.{} (expected {}.{})",
                version_major, version_minor, TLS_VERSION_MAJOR, TLS_VERSION_MINOR
            )));
        }

        let body_len = u16::from_be_bytes([header[3], header[4]]) as usize;
        let min_body = KID_SIZE + COUNTER_SIZE + TAG_SIZE;
        if body_len < min_body {
            return Err(TransportError::Protocol(format!(
                "frame body too small: {} (min {})",
                body_len, min_body
            )));
        }
        if body_len > MAX_FRAME_BODY {
            return Err(TransportError::FrameTooLarge(body_len, MAX_FRAME_BODY));
        }

        let mut body = vec![0u8; body_len];
        reader.read_exact(&mut body).await?;

        let kid = body[0];
        if kid != self.kid {
            return Err(TransportError::UnknownKeyId(kid));
        }

        let counter = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as u64;
        let ciphertext = &body[KID_SIZE + COUNTER_SIZE..];

        {
            let mut replay = self
                .recv_replay
                .lock()
                .map_err(|_| TransportError::Session("replay lock poisoned".into()))?;
            if !replay.check(counter) {
                return Err(TransportError::ReplayDetected(counter));
            }
        }

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        nonce_bytes[..PREFIX_SIZE].copy_from_slice(&self.recv_prefix);
        nonce_bytes[PREFIX_SIZE..].copy_from_slice(&(counter as u32).to_be_bytes());
        let nonce = Nonce::from_slice(&nonce_bytes);

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| TransportError::Crypto(e.to_string()))?;

        Ok(plaintext)
    }
}

/// Хранилище ключей с поддержкой ротации (minimal versioning для Phase 1).
/// Поддерживает 2+ активных ключа одновременно (dual-validation window).
pub struct KeyStore {
    keys: HashMap<u8, [u8; 32]>,
    active_kid: u8,
}

impl KeyStore {
    pub fn new(active_key: [u8; 32]) -> Self {
        let mut keys = HashMap::new();
        keys.insert(0, active_key);
        Self {
            keys,
            active_kid: 0,
        }
    }

    pub fn add_key(&mut self, kid: u8, key: [u8; 32]) {
        self.keys.insert(kid, key);
    }

    pub fn get_key(&self, kid: u8) -> Option<&[u8; 32]> {
        self.keys.get(&kid)
    }

    pub fn active_kid(&self) -> u8 {
        self.active_kid
    }

    pub fn active_key(&self) -> &[u8; 32] {
        self.keys
            .get(&self.active_kid)
            .expect("active key must exist")
    }

    /// Ротация: добавляет новый ключ и делает его активным.
    /// Старый ключ остаётся в хранилище для dual-validation.
    pub fn rotate(&mut self, new_kid: u8, new_key: [u8; 32]) {
        self.keys.insert(new_kid, new_key);
        self.active_kid = new_kid;
    }

    /// Удаляет старый ключ (после завершения dual-validation window).
    pub fn remove_key(&mut self, kid: u8) {
        if kid != self.active_kid {
            self.keys.remove(&kid);
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_frame() {
        let key = FrameCodec::generate_key();
        let prefix = FrameCodec::generate_prefix();
        let codec = FrameCodec::new(&key, prefix, prefix, 0);

        let original = b"GET https://web.telegram.org/ HTTP/1.1\r\nHost: web.telegram.org\r\n\r\n";

        let mut buf: Vec<u8> = Vec::new();
        codec.write_frame(&mut buf, original).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = codec.read_frame(&mut cursor).await.unwrap();

        assert_eq!(decoded, original);
    }

    #[tokio::test]
    async fn roundtrip_two_codecs() {
        let key = FrameCodec::generate_key();
        let c2s_prefix = FrameCodec::generate_prefix();
        let s2c_prefix = FrameCodec::generate_prefix();

        let client_codec = FrameCodec::new(&key, c2s_prefix, s2c_prefix, 0);
        let server_codec = FrameCodec::new(&key, s2c_prefix, c2s_prefix, 0);

        let original = b"client to server message";

        let mut buf: Vec<u8> = Vec::new();
        client_codec.write_frame(&mut buf, original).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = server_codec.read_frame(&mut cursor).await.unwrap();
        assert_eq!(decoded, original);
    }

    #[tokio::test]
    async fn rejects_bad_content_type() {
        let key = FrameCodec::generate_key();
        let codec = FrameCodec::new(&key, [0; 8], [0; 8], 0);

        let bad_frame = b"\xFF\x03\x03\x00\x15\x00\x00\x00\x00\x00deadbeefdeadbeef";
        let mut cursor = std::io::Cursor::new(&bad_frame[..]);
        let err = codec.read_frame(&mut cursor).await.unwrap_err();
        assert!(matches!(err, TransportError::Protocol(_)));
    }

    #[tokio::test]
    async fn rejects_replay() {
        let key = FrameCodec::generate_key();
        let prefix = FrameCodec::generate_prefix();
        let codec = FrameCodec::new(&key, prefix, prefix, 0);

        let mut buf: Vec<u8> = Vec::new();
        codec.write_frame(&mut buf, b"hello").await.unwrap();

        let mut cursor1 = std::io::Cursor::new(buf.clone());
        codec.read_frame(&mut cursor1).await.unwrap();

        let mut cursor2 = std::io::Cursor::new(buf);
        let err = codec.read_frame(&mut cursor2).await.unwrap_err();
        assert!(matches!(err, TransportError::ReplayDetected(_)));
    }

    #[tokio::test]
    async fn rejects_wrong_kid() {
        let key = FrameCodec::generate_key();
        let prefix = FrameCodec::generate_prefix();
        let writer_codec = FrameCodec::new(&key, prefix, prefix, 1);
        let reader_codec = FrameCodec::new(&key, prefix, prefix, 0);

        let mut buf: Vec<u8> = Vec::new();
        writer_codec.write_frame(&mut buf, b"hello").await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let err = reader_codec.read_frame(&mut cursor).await.unwrap_err();
        assert!(matches!(err, TransportError::UnknownKeyId(1)));
    }

    #[tokio::test]
    async fn rejects_oversized_payload() {
        let key = FrameCodec::generate_key();
        let codec = FrameCodec::new(&key, [0; 8], [0; 8], 0);

        let big = vec![0u8; MAX_PAYLOAD + 1];
        let mut buf: Vec<u8> = Vec::new();
        let err = codec.write_frame(&mut buf, &big).await.unwrap_err();
        assert!(matches!(err, TransportError::FrameTooLarge(_, _)));
    }

    #[tokio::test]
    async fn counter_increments_monotonically() {
        let key = FrameCodec::generate_key();
        let codec = FrameCodec::new(&key, [0; 8], [0; 8], 0);

        let mut buf: Vec<u8> = Vec::new();
        codec.write_frame(&mut buf, b"a").await.unwrap();
        assert_eq!(codec.send_counter(), 1);

        codec.write_frame(&mut buf, b"b").await.unwrap();
        assert_eq!(codec.send_counter(), 2);

        codec.write_frame(&mut buf, b"c").await.unwrap();
        assert_eq!(codec.send_counter(), 3);
    }

    #[test]
    fn test_keystore() {
        let key1 = FrameCodec::generate_key();
        let mut store = KeyStore::new(key1);
        assert_eq!(store.active_kid(), 0);
        assert_eq!(store.len(), 1);

        let key2 = FrameCodec::generate_key();
        store.rotate(1, key2);
        assert_eq!(store.active_kid(), 1);
        assert_eq!(store.len(), 2);

        store.remove_key(0);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_rekey_threshold() {
        assert!(REKEY_THRESHOLD < REKEY_HARD_LIMIT);
        assert!(REKEY_THRESHOLD < u32::MAX as u64);
    }
}
