/// Кастомный протокол поверх TLS.
///
/// Формат фрейма (бинарный, поверх уже установленного TLS-стрима):
///
///  ┌──────────┬────────────┬──────────┬─────────────────────┐
///  │  magic   │  version   │  length  │       payload       │
///  │  4 bytes │  1 byte    │  4 bytes │     N bytes         │
///  └──────────┴────────────┴──────────┴─────────────────────┘
///
/// magic   = 0x49_42_52_57  ("IBRW" — Isolated Browser)
/// version = 0x01
/// length  = длина payload в байтах (big-endian u32)
/// payload = зашифрованные данные (ChaCha20-Poly1305)
///
/// Перед payload идёт 12-байтовый nonce (случайный, per-frame).
/// Итого overhead на фрейм: 4 + 1 + 4 + 12 = 21 байт.

use bytes::{Buf, BufMut, BytesMut};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::RngCore;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use crate::error::TransportError;

const MAGIC: u32       = 0x49_42_52_57;
const VERSION: u8      = 0x01;
const HEADER_SIZE: usize = 4 + 1 + 4;   // magic + version + length
const NONCE_SIZE: usize  = 12;
const MAX_PAYLOAD: usize = 64 * 1024;    // 64 KB максимальный фрейм

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

        let payload_len = (NONCE_SIZE + ciphertext.len()) as u32;

        let mut buf = BytesMut::with_capacity(HEADER_SIZE + NONCE_SIZE + ciphertext.len());
        buf.put_u32(MAGIC);
        buf.put_u8(VERSION);
        buf.put_u32(payload_len);
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
        let mut header = [0u8; HEADER_SIZE];
        reader.read_exact(&mut header).await?;

        let mut cursor = &header[..];
        let magic = cursor.get_u32();
        if magic != MAGIC {
            return Err(TransportError::Protocol(format!(
                "bad magic: 0x{:08X}",
                magic
            )));
        }

        let version = cursor.get_u8();
        if version != VERSION {
            return Err(TransportError::Protocol(format!(
                "unsupported version: {}",
                version
            )));
        }

        let payload_len = cursor.get_u32() as usize;
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
    async fn rejects_bad_magic() {
        let key = FrameCodec::generate_key();
        let codec = FrameCodec::new(&key);

        // Собираем фрейм с неправильным magic
        let bad_frame = b"\xFF\xFF\xFF\xFF\x01\x00\x00\x00\x10deadbeefdeadbeef";
        let mut cursor = std::io::Cursor::new(&bad_frame[..]);
        let err = codec.read_frame(&mut cursor).await.unwrap_err();
        assert!(matches!(err, crate::error::TransportError::Protocol(_)));
    }
}
