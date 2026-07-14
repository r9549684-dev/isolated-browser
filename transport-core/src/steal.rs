/// Steal-oncall handshake with Forward Secrecy (ECDHE per session).
///
/// Архитектура:
/// 1. Клиент подключается к gateway IP:443
/// 2. TLS ClientHello с SNI = CDN-домен (например, cloudflare.com)
/// 3. Сервер завершает TLS handshake
/// 4. После TLS клиент отправляет auth frame (80 байт):
///    - Ephemeral X25519 public key (32 bytes)
///    - Auth token = HKDF(X25519(ephemeral_secret, server_static_public), "auth") (32 bytes)
///    - c2s_prefix: nonce prefix для client→server (8 bytes)
///    - s2c_prefix: nonce prefix для server→client (8 bytes)
/// 5. Сервер проверяет auth_token (constant-time):
///    - Вычисляет shared_secret = X25519(server_static_secret, client_ephemeral_public)
///    - Если token совпадает — генерирует fresh server ephemeral keypair
///    - Отправляет server_ephemeral_public (32 bytes) клиенту
///    - Если невалиден — закрывает соединение (unified error)
/// 6. Обе стороны вычисляют session_key = HKDF(X25519(client_ephemeral, server_ephemeral), "session")
/// 7. FrameCodec использует session_key для ChaCha20-Poly1305
///
/// Forward Secrecy: компрометация server_static_secret НЕ раскрывает прошлые
/// сессии, т.к. server_ephemeral_secret уничтожается после каждого соединения.
///
/// Nonce prefixes (c2s_prefix, s2c_prefix) — случайные 8 байт per-session,
/// гарантируют уникальность nonce при использовании counter-based nonce.
/// Разные prefix для каждого направления предотвращают nonce collision.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use x25519_dalek::{PublicKey, StaticSecret};
use rand::rngs::OsRng;
use rand::RngCore;
use hkdf::Hkdf;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use crate::error::TransportError;
use crate::protocol::FrameCodec;
use crate::protocol::{REKEY_INIT_MAGIC, REKEY_ACK_MAGIC};

pub const AUTH_FRAME_SIZE: usize = 80;
pub const AUTH_RESPONSE_SIZE: usize = 32;
pub const PUBLIC_KEY_SIZE: usize = 32;
pub const AUTH_TOKEN_SIZE: usize = 32;
pub const PREFIX_SIZE: usize = 8;

/// HKDF info labels для key separation (auth token vs session key).
const AUTH_INFO: &[u8] = b"isolated-browser-auth";
const SESSION_INFO: &[u8] = b"isolated-browser-session";

pub struct ClientAuth {
    pub session_key: [u8; 32],
    pub c2s_prefix: [u8; PREFIX_SIZE],
    pub s2c_prefix: [u8; PREFIX_SIZE],
}

/// Constant-time comparison для защиты от timing side-channel атак.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.ct_eq(b).into()
}

/// Client-side ECDHE handshake с forward secrecy.
///
/// Flow:
/// 1. Generate ephemeral X25519 keypair
/// 2. auth_token = HKDF(X25519(client_ephemeral, server_static_public), "auth")
/// 3. Send auth frame: ephemeral_public(32) + auth_token(32) + c2s_prefix(8) + s2c_prefix(8)
/// 4. Read server ephemeral public (32 bytes)
/// 5. session_key = HKDF(X25519(client_ephemeral, server_ephemeral), "session")
///
/// PFS: compromising server_static_secret does NOT reveal past sessions,
/// because server_ephemeral_secret is destroyed after each connection.
pub async fn client_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    server_static_public: &[u8; 32],
) -> Result<ClientAuth, TransportError> {
    // Используем StaticSecret (а не EphemeralSecret) т.к. diffie_hellman(&self)
    // заимствует, позволяя переиспользовать ключ для двух DH операций (auth + session).
    // Ключ уничтожается (zeroized) при выходе из функции — PFS сохраняется.
    let client_secret = StaticSecret::random_from_rng(OsRng);
    let client_public = PublicKey::from(&client_secret);

    let server_static = PublicKey::from(*server_static_public);
    let auth_shared = client_secret.diffie_hellman(&server_static);
    let mut auth_token = [0u8; AUTH_TOKEN_SIZE];
    Hkdf::<Sha256>::new(None, auth_shared.as_bytes())
        .expand(AUTH_INFO, &mut auth_token)
        .expect("HKDF expand failed");

    let mut c2s_prefix = [0u8; PREFIX_SIZE];
    let mut s2c_prefix = [0u8; PREFIX_SIZE];
    OsRng.fill_bytes(&mut c2s_prefix);
    OsRng.fill_bytes(&mut s2c_prefix);

    let mut frame = [0u8; AUTH_FRAME_SIZE];
    frame[..32].copy_from_slice(client_public.as_bytes());
    frame[32..64].copy_from_slice(&auth_token);
    frame[64..72].copy_from_slice(&c2s_prefix);
    frame[72..80].copy_from_slice(&s2c_prefix);
    stream.write_all(&frame).await?;
    stream.flush().await?;

    let mut response = [0u8; AUTH_RESPONSE_SIZE];
    stream.read_exact(&mut response).await?;

    let server_ephemeral = PublicKey::from(response);
    let session_shared = client_secret.diffie_hellman(&server_ephemeral);
    let mut session_key = [0u8; 32];
    Hkdf::<Sha256>::new(None, session_shared.as_bytes())
        .expand(SESSION_INFO, &mut session_key)
        .expect("HKDF expand failed");

    Ok(ClientAuth {
        session_key,
        c2s_prefix,
        s2c_prefix,
    })
}

/// Server-side ECDHE: verify auth_token + derive session key with fresh ephemeral.
///
/// Возвращает Some((server_ephemeral_public, session_key)) если auth OK,
/// None если auth failed. server_ephemeral_public (32 bytes) нужно отправить
/// клиенту как auth response.
///
/// PFS: server_ephemeral_secret генерируется freshly per-connection и
/// уничтожается (zeroized) при возврате из функции.
pub fn server_derive_session(
    server_static_secret: &[u8; 32],
    client_ephemeral_public: &[u8; 32],
    received_auth_token: &[u8; 32],
) -> Option<([u8; 32], [u8; 32])> {
    let server_static = StaticSecret::from(*server_static_secret);
    let client_pub = PublicKey::from(*client_ephemeral_public);
    let auth_shared = server_static.diffie_hellman(&client_pub);

    let mut expected_token = [0u8; AUTH_TOKEN_SIZE];
    Hkdf::<Sha256>::new(None, auth_shared.as_bytes())
        .expand(AUTH_INFO, &mut expected_token)
        .expect("HKDF expand failed");

    if !constant_time_eq(&expected_token, received_auth_token) {
        return None;
    }

    // Fresh ephemeral keypair per-connection — основа forward secrecy.
    // Уничтожается (zeroized) при выходе из функции.
    let server_ephemeral_secret = StaticSecret::random_from_rng(OsRng);
    let server_ephemeral_public = PublicKey::from(&server_ephemeral_secret);

    let session_shared = server_ephemeral_secret.diffie_hellman(&client_pub);
    let mut session_key = [0u8; 32];
    Hkdf::<Sha256>::new(None, session_shared.as_bytes())
        .expand(SESSION_INFO, &mut session_key)
        .expect("HKDF expand failed");

    Some((*server_ephemeral_public.as_bytes(), session_key))
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

/// Устанавливает сессию с gateway: TLS handshake + ECDHE + FrameCodec.
/// Используется клиентом (proxy.rs) и stress test.
pub async fn establish_session(
    tls_stream: &mut TlsStream<TcpStream>,
    server_public: &[u8; 32],
    kid: u8,
) -> Result<FrameCodec, TransportError> {
    let client_auth = client_handshake(tls_stream, server_public).await?;

    let codec = FrameCodec::new(
        &client_auth.session_key,
        client_auth.c2s_prefix,
        client_auth.s2c_prefix,
        kid,
    );

    Ok(codec)
}

/// Инициирует rekey: сервер генерирует новый ключ и отправляет REKEY_INIT
/// через существующий FrameCodec (зашифрован старым ключом).
/// Формат plaintext: REKEY_INIT_MAGIC(6) + new_kid(1) + new_key(32) = 39 bytes.
pub async fn send_rekey_init<W: AsyncWrite + Unpin>(
    codec: &FrameCodec,
    writer: &mut W,
    new_kid: u8,
    new_key: &[u8; 32],
) -> Result<(), TransportError> {
    let mut payload = Vec::with_capacity(REKEY_INIT_MAGIC.len() + 1 + 32);
    payload.extend_from_slice(REKEY_INIT_MAGIC);
    payload.push(new_kid);
    payload.extend_from_slice(new_key);
    codec.write_frame(writer, &payload).await
}

/// Клиент читает REKEY_INIT, отправляет REKEY_ACK (под OLD ключом), затем применяет rekey.
/// Формат REKEY_INIT plaintext: REKEY_INIT_MAGIC(6) + new_kid(1) + new_key(32).
/// Формат REKEY_ACK plaintext: REKEY_ACK_MAGIC(9) + new_kid(1).
///
/// Важно: ACK отправляется ДО start_rekey (под old key), т.к. сервер ещё не сделал rekey.
pub async fn client_handle_rekey<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    codec: &FrameCodec,
    reader: &mut R,
    writer: &mut W,
) -> Result<(u8, [u8; 32]), TransportError> {
    let frame = codec.read_frame(reader).await?;
    if frame.len() < REKEY_INIT_MAGIC.len() + 1 + 32 {
        return Err(TransportError::Protocol("REKEY_INIT too short".into()));
    }
    if &frame[..REKEY_INIT_MAGIC.len()] != REKEY_INIT_MAGIC {
        return Err(TransportError::Protocol("bad REKEY_INIT magic".into()));
    }
    let new_kid = frame[REKEY_INIT_MAGIC.len()];
    let mut new_key = [0u8; 32];
    new_key.copy_from_slice(&frame[REKEY_INIT_MAGIC.len() + 1..REKEY_INIT_MAGIC.len() + 1 + 32]);

    // ACK под OLD ключом (до start_rekey).
    let mut ack = Vec::with_capacity(REKEY_ACK_MAGIC.len() + 1);
    ack.extend_from_slice(REKEY_ACK_MAGIC);
    ack.push(new_kid);
    codec.write_frame(writer, &ack).await?;

    // Теперь применяем rekey (overlap window активен).
    codec.start_rekey(new_kid, &new_key);

    Ok((new_kid, new_key))
}

/// Сервер читает REKEY_ACK и применяет rekey (start_rekey).
/// Формат REKEY_ACK plaintext: REKEY_ACK_MAGIC(9) + new_kid(1).
pub async fn server_handle_rekey_ack<R: AsyncRead + Unpin>(
    codec: &FrameCodec,
    reader: &mut R,
    new_kid: u8,
    new_key: &[u8; 32],
) -> Result<(), TransportError> {
    let frame = codec.read_frame(reader).await?;
    if frame.len() < REKEY_ACK_MAGIC.len() + 1 {
        return Err(TransportError::Protocol("REKEY_ACK too short".into()));
    }
    if &frame[..REKEY_ACK_MAGIC.len()] != REKEY_ACK_MAGIC {
        return Err(TransportError::Protocol("bad REKEY_ACK magic".into()));
    }
    let ack_kid = frame[REKEY_ACK_MAGIC.len()];
    if ack_kid != new_kid {
        return Err(TransportError::Protocol(format!(
            "REKEY_ACK kid mismatch: {} != {}",
            ack_kid, new_kid
        )));
    }
    codec.start_rekey(new_kid, new_key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthKeyPair;

    #[test]
    fn test_auth_frame_size() {
        assert_eq!(AUTH_FRAME_SIZE, 80);
        assert_eq!(AUTH_RESPONSE_SIZE, 32);
    }

    #[test]
    fn test_ecdhe_session_key_match() {
        let server_keypair = AuthKeyPair::generate();
        let client_secret = StaticSecret::random_from_rng(OsRng);
        let client_public = PublicKey::from(&client_secret);

        let auth_shared = client_secret.diffie_hellman(&server_keypair.public);
        let mut auth_token = [0u8; AUTH_TOKEN_SIZE];
        Hkdf::<Sha256>::new(None, auth_shared.as_bytes())
            .expand(AUTH_INFO, &mut auth_token)
            .expect("HKDF expand failed");

        let result = server_derive_session(
            server_keypair.secret.as_bytes(),
            client_public.as_bytes(),
            &auth_token,
        );
        let (server_ephemeral_public, server_session_key) = result.expect("auth should succeed");

        let server_eph = PublicKey::from(server_ephemeral_public);
        let client_session_shared = client_secret.diffie_hellman(&server_eph);
        let mut client_session_key = [0u8; 32];
        Hkdf::<Sha256>::new(None, client_session_shared.as_bytes())
            .expand(SESSION_INFO, &mut client_session_key)
            .expect("HKDF expand failed");

        assert_eq!(client_session_key, server_session_key);
    }

    #[test]
    fn test_different_sessions_different_keys() {
        let server_keypair = AuthKeyPair::generate();

        let mut keys = Vec::new();
        for _ in 0..3 {
            let client_secret = StaticSecret::random_from_rng(OsRng);
            let client_public = PublicKey::from(&client_secret);

            let auth_shared = client_secret.diffie_hellman(&server_keypair.public);
            let mut auth_token = [0u8; AUTH_TOKEN_SIZE];
            Hkdf::<Sha256>::new(None, auth_shared.as_bytes())
                .expand(AUTH_INFO, &mut auth_token)
                .expect("HKDF expand failed");

            let (_, session_key) = server_derive_session(
                server_keypair.secret.as_bytes(),
                client_public.as_bytes(),
                &auth_token,
            )
            .expect("auth should succeed");

            keys.push(session_key);
        }

        assert_ne!(keys[0], keys[1]);
        assert_ne!(keys[1], keys[2]);
        assert_ne!(keys[0], keys[2]);
    }

    #[test]
    fn test_invalid_auth_token_rejected() {
        let server_keypair = AuthKeyPair::generate();
        let client_secret = StaticSecret::random_from_rng(OsRng);
        let client_public = PublicKey::from(&client_secret);

        let bad_token = [0xFFu8; 32];

        let result = server_derive_session(
            server_keypair.secret.as_bytes(),
            client_public.as_bytes(),
            &bad_token,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_session_key_differs_from_auth_token() {
        let server_keypair = AuthKeyPair::generate();
        let client_secret = StaticSecret::random_from_rng(OsRng);
        let client_public = PublicKey::from(&client_secret);

        let auth_shared = client_secret.diffie_hellman(&server_keypair.public);
        let mut auth_token = [0u8; AUTH_TOKEN_SIZE];
        Hkdf::<Sha256>::new(None, auth_shared.as_bytes())
            .expand(AUTH_INFO, &mut auth_token)
            .expect("HKDF expand failed");

        let (_, session_key) = server_derive_session(
            server_keypair.secret.as_bytes(),
            client_public.as_bytes(),
            &auth_token,
        )
        .expect("auth should succeed");

        assert_ne!(auth_token, session_key);
    }

    #[test]
    fn test_prefixes_are_random() {
        let mut c2s_1 = [0u8; PREFIX_SIZE];
        let mut c2s_2 = [0u8; PREFIX_SIZE];
        OsRng.fill_bytes(&mut c2s_1);
        OsRng.fill_bytes(&mut c2s_2);

        assert_ne!(c2s_1, c2s_2);
    }

    #[tokio::test]
    async fn test_client_handshake_roundtrip() {
        let server_keypair = AuthKeyPair::generate();
        let server_secret = *server_keypair.secret.as_bytes();
        let server_public = *server_keypair.public.as_bytes();

        // Один duplex: client и server стороны читают/пишут друг другу.
        let (mut client_stream, mut server_stream) = tokio::io::duplex(4096);

        let client_handle = tokio::spawn(async move {
            client_handshake(&mut client_stream, &server_public).await
        });

        let server_handle = tokio::spawn(async move {
            let (ephemeral, token, _c2s, _s2c) = read_auth_frame(&mut server_stream).await?;
            match server_derive_session(&server_secret, &ephemeral, &token) {
                Some((server_eph_pub, session_key)) => {
                    server_stream.write_all(&server_eph_pub).await?;
                    server_stream.flush().await?;
                    Ok::<_, TransportError>(Some(session_key))
                }
                None => Ok(None),
            }
        });

        let client_auth = client_handle.await.unwrap().expect("client handshake");
        let server_result = server_handle.await.unwrap().expect("server handshake");
        let server_session_key = server_result.expect("auth should succeed");

        assert_eq!(client_auth.session_key, server_session_key);
    }
}
