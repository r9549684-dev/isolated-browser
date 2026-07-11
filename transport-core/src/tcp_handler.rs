/// TCP-level обработка TLS ClientHello для защиты от active probing.
///
/// Архитектура (как в REALITY):
/// 1. Принять TCP соединение
/// 2. Прочитать ClientHello (первые байты TLS handshake)
/// 3. Извлечь auth token из ClientHello (из session_id или custom extension)
/// 4. Если auth валиден — продолжить TLS handshake через rustls
/// 5. Если auth невалиден — проксировать сырые TCP байты на реальный CDN (fallback)
///
/// Это защищает от active probing:
/// - ТСПУ видит настоящий TLS ClientHello
/// - При невалидном auth — байт-в-байт проксирование на CDN (тот же сертификат/JA3S)
/// - Нет "второго" handshake, нет отличий от прямого соединения

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use std::io;

pub const TLS_RECORD_HEADER_SIZE: usize = 5;
pub const TLS_HANDSHAKE_HEADER_SIZE: usize = 4;
pub const CLIENT_HELLO_MIN_SIZE: usize = 38;

/// Извлекает auth token из TLS ClientHello.
/// 
/// Auth token может быть в:
/// - session_id (первые 32 байта)
/// - custom TLS extension (например, extension ID 0x4942)
/// 
/// Возвращает: (auth_token, remaining_bytes)
pub fn extract_auth_from_clienthello(data: &[u8]) -> Option<([u8; 32], Vec<u8>)> {
    if data.len() < TLS_RECORD_HEADER_SIZE {
        return None;
    }
    
    // Парсим TLS record layer
    let content_type = data[0];
    if content_type != 0x16 {
        // Не handshake record
        return None;
    }
    
    let _version_major = data[1];
    let _version_minor = data[2];
    let record_length = u16::from_be_bytes([data[3], data[4]]) as usize;
    
    if data.len() < TLS_RECORD_HEADER_SIZE + record_length {
        return None;
    }
    
    let handshake_data = &data[TLS_RECORD_HEADER_SIZE..];
    
    // Парсим Handshake layer
    let handshake_type = handshake_data[0];
    if handshake_type != 0x01 {
        // Не ClientHello
        return None;
    }
    
    let handshake_length = u32::from_be_bytes([0, handshake_data[1], handshake_data[2], handshake_data[3]]) as usize;
    
    if handshake_data.len() < TLS_HANDSHAKE_HEADER_SIZE + handshake_length {
        return None;
    }
    
    let client_hello_data = &handshake_data[TLS_HANDSHAKE_HEADER_SIZE..];
    
    // Парсим ClientHello
    if client_hello_data.len() < CLIENT_HELLO_MIN_SIZE {
        return None;
    }
    
    let _client_version = u16::from_be_bytes([client_hello_data[0], client_hello_data[1]]);
    let _client_random = &client_hello_data[2..34];
    
    // session_id_length
    let session_id_length = client_hello_data[34] as usize;
    
    if session_id_length >= 32 {
        // Извлекаем auth token из session_id (первые 32 байта)
        let session_id = &client_hello_data[35..35 + session_id_length];
        let mut auth_token = [0u8; 32];
        auth_token.copy_from_slice(&session_id[..32]);
        
        return Some((auth_token, data.to_vec()));
    }
    
    // Если session_id слишком короткий, ищем custom extension
    // (упрощенная реализация — в реальности нужно парсить все extensions)
    
    None
}

/// Проксирует сырые TCP байты на реальный CDN (fallback).
/// Это байт-в-байт проксирование без модификации данных.
pub async fn fallback_tcp_proxy(
    client: TcpStream,
    cdn_addr: &str,
    initial_data: &[u8],
) -> io::Result<()> {
    let mut cdn = TcpStream::connect(cdn_addr).await?;
    
    // Отправляем начальные данные (ClientHello) на CDN
    cdn.write_all(initial_data).await?;
    cdn.flush().await?;
    
    // Простой bidirectional relay
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut cdn_read, mut cdn_write) = tokio::io::split(cdn);
    
    let client_to_cdn = tokio::io::copy(&mut client_read, &mut cdn_write);
    let cdn_to_client = tokio::io::copy(&mut cdn_read, &mut client_write);
    
    tokio::select! {
        result = client_to_cdn => {
            if let Err(e) = result {
                tracing::debug!("fallback client->cdn error: {}", e);
            }
        }
        result = cdn_to_client => {
            if let Err(e) = result {
                tracing::debug!("fallback cdn->client error: {}", e);
            }
        }
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_auth_from_invalid_data() {
        let data = vec![0u8; 10];
        assert!(extract_auth_from_clienthello(&data).is_none());
    }

    #[test]
    fn test_extract_auth_from_non_handshake() {
        let mut data = vec![0u8; 100];
        data[0] = 0x17; // Application Data, не Handshake
        assert!(extract_auth_from_clienthello(&data).is_none());
    }
}
