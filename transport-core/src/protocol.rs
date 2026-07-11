/// Протокол с маскировкой под TLS 1.3 Application Data.
///
/// Каждый фрейм выглядит как TLS record:
///
///  ┌──────────┬────────────┬──────────┬─────────────────────────────────┐
///  │  0x17    │  0x03 0x03 │  length  │         payload                 │
///  │  1 byte  │  2 bytes   │  2 bytes │        N bytes                  │
///  └──────────┴────────────┴──────────┴─────────────────────────────────┘
///
/// 0x17 = TLS Application Data content type
/// 0x03 0x03 = TLS 1.2 version (для совместимости с DPI)
/// length = длина payload (big-endian u16)
/// payload = nonce (12 bytes) + ciphertext (ChaCha20-Poly1305)
///
/// Итого overhead на фрейм: 5 байт (TLS record header) + 12 байт (nonce) = 17 байт.

use bytes::{Buf, BufMut, BytesMut};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::RngCore;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use crate::error::TransportError;

const TLS_CONTENT_TYPE: u8 = 0x17;
const TLS_VERSION_MAJOR: u8 = 0x03;
const TLS_VERSION_MINOR: u8 = 0x03;
const TLS_RECORD_HEADER_SIZE: usize = 5;
const NONCE_SIZE: usize = 12;
const MAX_PAYLOAD: usize = 16 * 1024;

pub struct FrameCodec {
    cipher: ChaCha20Poly1305,
}

impl FrameCodec {
    /// Создаёт кодек с 256-битным ключом.
    /// В реальном деплое ключ согласовывается через handshake (будущий шаг).
    pub fn new(key: &[u8; 32]) -> Self {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
        Self { cipher }
    }

    /// Генерирует случайный 32-байтовый ключ (для тестов).
    pub fn generate_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    }

    /// Кодирует plaintext в бинарный фрейм и пишет в writer.
    pub async fn write_frame<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        plaintext: &[u8],
    ) -> Result<(), TransportError> {
        if plaintext.len() > MAX_PAYLOAD {
            return Err(TransportError::Protocol(format!(
                "payload too large: {} > {}",
                plaintext.len(),
                MAX_PAYLOAD
            )));
        }

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self.cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| TransportError::Crypto(e.to_string()))?;

        let payload_len = (NONCE_SIZE + ciphertext.len()) as u16;

        let mut buf = BytesMut::with_capacity(TLS_RECORD_HEADER_SIZE + NONCE_SIZE + ciphertext.len());
        buf.put_u8(TLS_CONTENT_TYPE);
        buf.put_u8(TLS_VERSION_MAJOR);
        buf.put_u8(TLS_VERSION_MINOR);
        buf.put_u16(payload_len);
        buf.put_slice(&nonce_bytes);
        buf.put_slice(&ciphertext);

        writer.write_all(&buf).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Читает один фрейм из reader, расшифровывает и возвращает plaintext.
    pub async fn read_frame<R: AsyncRead + Unpin>(
        &self,
        reader: &mut R,
    ) -> Result<Vec<u8>, TransportError> {
        let mut header = [0u8; TLS_RECORD_HEADER_SIZE];
        reader.read_exact(&mut header).await?;

        let mut cursor = &header[..];
        let content_type = cursor.get_u8();
        if content_type != TLS_CONTENT_TYPE {
            return Err(TransportError::Protocol(format!(
                "bad content type: 0x{:02X} (expected 0x{:02X})",
                content_type, TLS_CONTENT_TYPE
            )));
        }

        let version_major = cursor.get_u8();
        let version_minor = cursor.get_u8();
        if version_major != TLS_VERSION_MAJOR || version_minor != TLS_VERSION_MINOR {
            return Err(TransportError::Protocol(format!(
                "bad TLS version: {}.{} (expected {}.{}",
                version_major, version_minor, TLS_VERSION_MAJOR, TLS_VERSION_MINOR
            )));
        }

        let payload_len = cursor.get_u16() as usize;
        if payload_len < NONCE_SIZE || payload_len > MAX_PAYLOAD + NONCE_SIZE + 16 {
            return Err(TransportError::Protocol(format!(
                "invalid payload length: {}",
                payload_len
            )));
        }

        let mut payload = vec![0u8; payload_len];
        reader.read_exact(&mut payload).await?;

        let nonce = Nonce::from_slice(&payload[..NONCE_SIZE]);
        let ciphertext = &payload[NONCE_SIZE..];

        let plaintext = self.cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| TransportError::Crypto(e.to_string()))?;

        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_frame() {
        let key = FrameCodec::generate_key();
        let codec = FrameCodec::new(&key);

        let original = b"GET https://web.telegram.org/ HTTP/1.1\r\nHost: web.telegram.org\r\n\r\n";

        let mut buf: Vec<u8> = Vec::new();
        codec.write_frame(&mut buf, original).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded = codec.read_frame(&mut cursor).await.unwrap();

        assert_eq!(decoded, original);
    }

    #[tokio::test]
    async fn rejects_bad_content_type() {
        let key = FrameCodec::generate_key();
        let codec = FrameCodec::new(&key);

        let bad_frame = b"\xFF\x03\x03\x00\x10deadbeefdeadbeef";
        let mut cursor = std::io::Cursor::new(&bad_frame[..]);
        let err = codec.read_frame(&mut cursor).await.unwrap_err();
        assert!(matches!(err, crate::error::TransportError::Protocol(_)));
    }
}
