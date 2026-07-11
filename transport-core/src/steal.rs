/// Steal-oncall handshake для обхода active probing.
///
/// Архитектура:
/// 1. Клиент подключается к gateway IP:443
/// 2. TLS ClientHello с SNI = CDN-домен (например, cloudflare.com)
/// 3. Сервер должен иметь валидный сертификат для этого CDN-домена
/// 4. После TLS handshake клиент отправляет:
///    - Ephemeral X25519 public key (32 bytes)
///    - Auth token = HKDF(X25519(ephemeral_secret, server_public)) (32 bytes)
/// 5. Сервер проверяет token:
///    - Вычисляет shared_secret = X25519(server_secret, ephemeral_public)
///    - Если token совпадает — обрабатывает запрос
///    - Если невалиден — закрывает соединение (stealth mode)
///
/// Это защищает от active probing:
/// - DPI видит TLS handshake к CDN (SNI = cloudflare.com)
/// - Сертификат от Let's Encrypt для CDN-домена
/// - Без auth token соединение закрывается
/// - Active probe получает TLS alert или закрытие соединения

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use x25519_dalek::{EphemeralSecret, PublicKey};
use rand::rngs::OsRng;
use crate::error::TransportError;

pub const AUTH_FRAME_SIZE: usize = 64;
pub const PUBLIC_KEY_SIZE: usize = 32;
pub const AUTH_TOKEN_SIZE: usize = 32;

pub struct ClientAuth {
    pub ephemeral_public: [u8; 32],
    pub auth_token: [u8; 32],
}

pub fn generate_client_auth(server_public: &[u8; 32]) -> ClientAuth {
    let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    
    let server_public_key = PublicKey::from(*server_public);
    let shared_secret = ephemeral_secret.diffie_hellman(&server_public_key);
    
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(None, shared_secret.as_bytes());
    let mut auth_token = [0u8; 32];
    hkdf.expand(b"isolated-browser-auth", &mut auth_token)
        .expect("HKDF expand failed");
    
    ClientAuth {
        ephemeral_public: *ephemeral_public.as_bytes(),
        auth_token,
    }
}

pub async fn send_auth_frame(
    tls_stream: &mut TlsStream<TcpStream>,
    server_public: &[u8; 32],
) -> Result<(), TransportError> {
    let client_auth = generate_client_auth(server_public);
    
    let mut frame = [0u8; AUTH_FRAME_SIZE];
    frame[..32].copy_from_slice(&client_auth.ephemeral_public);
    frame[32..].copy_from_slice(&client_auth.auth_token);
    
    tls_stream.write_all(&frame).await?;
    tls_stream.flush().await?;
    
    Ok(())
}

pub async fn read_auth_frame(
    tls_stream: &mut TlsStream<TcpStream>,
) -> Result<([u8; 32], [u8; 32]), TransportError> {
    let mut frame = [0u8; AUTH_FRAME_SIZE];
    tls_stream.read_exact(&mut frame).await?;
    
    let mut ephemeral_public = [0u8; 32];
    let mut auth_token = [0u8; 32];
    ephemeral_public.copy_from_slice(&frame[..32]);
    auth_token.copy_from_slice(&frame[32..]);
    
    Ok((ephemeral_public, auth_token))
}

pub fn verify_server_auth(
    server_secret: &[u8; 32],
    ephemeral_public: &[u8; 32],
    received_token: &[u8; 32],
) -> bool {
    let server_static = x25519_dalek::StaticSecret::from(*server_secret);
    let client_public = PublicKey::from(*ephemeral_public);
    let shared_secret = server_static.diffie_hellman(&client_public);
    
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(None, shared_secret.as_bytes());
    let mut expected_token = [0u8; 32];
    hkdf.expand(b"isolated-browser-auth", &mut expected_token)
        .expect("HKDF expand failed");
    
    // Constant-time comparison (защита от timing side-channel)
    constant_time_eq(&expected_token, received_token)
}

/// Constant-time comparison для защиты от timing side-channel атак
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthKeyPair;

    #[test]
    fn test_auth_frame_size() {
        assert_eq!(AUTH_FRAME_SIZE, 64);
    }

    #[test]
    fn test_client_server_auth() {
        let server_keypair = AuthKeyPair::generate();
        
        let client_auth = generate_client_auth(server_keypair.public.as_bytes());
        
        let verified = verify_server_auth(
            server_keypair.secret.as_bytes(),
            &client_auth.ephemeral_public,
            &client_auth.auth_token,
        );
        
        assert!(verified);
    }
}
