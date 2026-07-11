/// X25519 аутентификация для steal-TLS handshake.
///
/// Архитектура:
/// 1. Клиент генерирует ephemeral X25519 keypair
/// 2. Клиент отправляет public key в TLS extension (custom extension ID)
/// 3. Сервер проверяет public key against whitelist (или использует shared secret для derive)
/// 4. Если auth успешен — продолжает handshake
/// 5. Если auth провален — fallback на реальный сайт (steal-oncall)
///
/// Это защищает от active probing: DPI видит настоящий TLS handshake к CDN,
/// но только авторизованные клиенты могут установить соединение с gateway.

use x25519_dalek::{PublicKey, StaticSecret};
use hkdf::Hkdf;
use sha2::Sha256;
use rand::rngs::OsRng;

pub const AUTH_EXTENSION_ID: u16 = 0x4942;
pub const AUTH_KEY_SIZE: usize = 32;

pub struct AuthKeyPair {
    pub secret: StaticSecret,
    pub public: PublicKey,
}

impl AuthKeyPair {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn from_bytes(secret_bytes: &[u8; 32]) -> Self {
        let secret = StaticSecret::from(*secret_bytes);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn derive_shared_key(&self, peer_public: &PublicKey) -> [u8; 32] {
        let shared_secret = self.secret.diffie_hellman(peer_public);
        
        let hkdf = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        let mut derived_key = [0u8; 32];
        hkdf.expand(b"isolated-browser-auth", &mut derived_key)
            .expect("HKDF expand failed");
        
        derived_key
    }
}

pub fn verify_auth_token(
    client_public: &[u8; 32],
    server_secret: &[u8; 32],
    expected_token: &[u8; 32],
) -> bool {
    let server_keypair = AuthKeyPair::from_bytes(server_secret);
    let client_public_key = PublicKey::from(*client_public);
    let derived = server_keypair.derive_shared_key(&client_public_key);
    
    derived == *expected_token
}

pub fn generate_auth_token(
    client_secret: &[u8; 32],
    server_public: &[u8; 32],
) -> [u8; 32] {
    let client_keypair = AuthKeyPair::from_bytes(client_secret);
    let server_public_key = PublicKey::from(*server_public);
    client_keypair.derive_shared_key(&server_public_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_exchange() {
        let client = AuthKeyPair::generate();
        let server = AuthKeyPair::generate();

        let client_shared = client.derive_shared_key(&server.public);
        let server_shared = server.derive_shared_key(&client.public);

        assert_eq!(client_shared, server_shared);
    }

    #[test]
    fn test_auth_token() {
        let client = AuthKeyPair::generate();
        let server = AuthKeyPair::generate();

        let token = generate_auth_token(
            client.secret.as_bytes(),
            server.public.as_bytes(),
        );

        let verified = verify_auth_token(
            client.public.as_bytes(),
            server.secret.as_bytes(),
            &token,
        );

        assert!(verified);
    }
}
