/// Steal-oncall handshake для обхода active probing.
///
/// Архитектура:
/// 1. Клиент подключается к gateway IP:443
/// 2. TLS ClientHello с SNI = CDN-домен (например, cloudflare.com)
/// 3. Сервер завершает TLS handshake
/// 4. После TLS клиент отправляет auth frame (80 байт):
///    - Ephemeral X25519 public key (32 bytes)
///    - Auth token = HKDF(X25519(ephemeral_secret, server_public)) (32 bytes)
///    - c2s_prefix: nonce prefix для client→server (8 bytes)
///    - s2c_prefix: nonce prefix для server→client (8 bytes)
/// 5. Сервер проверяет token:
///    - Вычисляет shared_secret = X25519(server_secret, ephemeral_public)
///    - Если token совпадает — обрабатывает запрос
///    - Если невалиден — закрывает соединение
///
/// Nonce prefixes (c2s_prefix, s2c_prefix) — случайные 8 байт per-session,
/// гарантируют уникальность nonce при использовании counter-based nonce.
/// Разные prefix для каждого направления предотвращают nonce collision.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use x25519_dalek::{EphemeralSecret, PublicKey};
use rand::rngs::OsRng;
use rand::RngCore;
use crate::error::TransportError;
use crate::protocol::FrameCodec;

pub const AUTH_FRAME_SIZE: usize = 80;
pub const PUBLIC_KEY_SIZE: usize = 32;
pub const AUTH_TOKEN_SIZE: usize = 32;
pub const PREFIX_SIZE: usize = 8;

pub struct ClientAuth {
    pub ephemeral_public: [u8; 32],
    pub auth_token: [u8; 32],
    pub c2s_prefix: [u8; PREFIX_SIZE],
    pub s2c_prefix: [u8; PREFIX_SIZE],
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

    let mut c2s_prefix = [0u8; PREFIX_SIZE];
    let mut s2c_prefix = [0u8; PREFIX_SIZE];
    OsRng.fill_bytes(&mut c2s_prefix);
    OsRng.fill_bytes(&mut s2c_prefix);

    ClientAuth {
        ephemeral_public: *ephemeral_public.as_bytes(),
        auth_token,
        c2s_prefix,
        s2c_prefix,
    }
}

pub async fn send_auth_frame(
    tls_stream: &mut TlsStream<TcpStream>,
    server_public: &[u8; 32],
) -> Result<ClientAuth, TransportError> {
    let client_auth = generate_client_auth(server_public);

    let mut frame = [0u8; AUTH_FRAME_SIZE];
    frame[..32].copy_from_slice(&client_auth.ephemeral_public);
    frame[32..64].copy_from_slice(&client_auth.auth_token);
    frame[64..72].copy_from_slice(&client_auth.c2s_prefix);
    frame[72..80].copy_from_slice(&client_auth.s2c_prefix);

    tls_stream.write_all(&frame).await?;
    tls_stream.flush().await?;

    Ok(client_auth)
}

/// Читает auth frame после TLS handshake.
/// Возвращает (ephemeral_public, auth_token, c2s_prefix, s2c_prefix).
pub async fn read_auth_frame<S: AsyncRead + Unpin>(
    tls_stream: &mut S,
) -> Result<([u8; 32], [u8; 32], [u8; 8], [u8; 8]), TransportError> {
    let mut frame = [0u8; AUTH_FRAME_SIZE];
    tls_stream.read_exact(&mut frame).await?;

    let mut ephemeral_public = [0u8; 32];
    let mut auth_token = [0u8; 32];
    let mut c2s_prefix = [0u8; 8];
    let mut s2c_prefix = [0u8; 8];

    ephemeral_public.copy_from_slice(&frame[..32]);
    auth_token.copy_from_slice(&frame[32..64]);
    c2s_prefix.copy_from_slice(&frame[64..72]);
    s2c_prefix.copy_from_slice(&frame[72..80]);

    Ok((ephemeral_public, auth_token, c2s_prefix, s2c_prefix))
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

    constant_time_eq(&expected_token, received_token)
}

/// Constant-time comparison для защиты от timing side-channel атак.
/// Использует subtle crate.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// Устанавливает сессию с gateway: TLS handshake + auth frame + FrameCodec.
/// Используется клиентом (proxy.rs) и stress test.
pub async fn establish_session(
    tls_stream: &mut TlsStream<TcpStream>,
    server_public: &[u8; 32],
    key: &[u8; 32],
    kid: u8,
) -> Result<FrameCodec, TransportError> {
    let client_auth = send_auth_frame(tls_stream, server_public).await?;

    let codec = FrameCodec::new(
        key,
        client_auth.c2s_prefix,
        client_auth.s2c_prefix,
        kid,
    );

    Ok(codec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthKeyPair;

    #[test]
    fn test_auth_frame_size() {
        assert_eq!(AUTH_FRAME_SIZE, 80);
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

    #[test]
    fn test_prefixes_are_random() {
        let server_keypair = AuthKeyPair::generate();
        let auth1 = generate_client_auth(server_keypair.public.as_bytes());
        let auth2 = generate_client_auth(server_keypair.public.as_bytes());

        assert_ne!(auth1.c2s_prefix, auth2.c2s_prefix);
        assert_ne!(auth1.s2c_prefix, auth2.s2c_prefix);
    }
}
