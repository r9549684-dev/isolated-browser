use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Authentication failed")]
    AuthFailed,
    #[error("Token expired")]
    TokenExpired,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Missing claims")]
    MissingClaims,
}

/// Унифицированная ошибка — не раскрывает причину отказа клиенту.
/// Все ошибки аутентификации возвращаются как AuthFailed.
/// Детали логируются сервером, но не отправляются клиенту.
impl AuthError {
    /// Возвращает унифицированное сообщение для клиента (anti timing/oracle leak).
    pub fn client_message(&self) -> &'static str {
        "ERR"
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub user_id: String,
    pub tier: String,
    pub rate_limit_bps: u64,
    pub exp: i64,
    pub iat: i64,
}

/// Менеджер ключей с поддержкой versioning (2 активных ключа).
/// kid (key id) — 1 байт, идентифицирует активный ключ.
/// При ротации: новый ключ добавляется, оба валидны до истечения dual-validation window.
pub struct AuthManager {
    jwt_secret: String,
    hmac_keys: Vec<Vec<u8>>,
    active_kid: u8,
}

impl AuthManager {
    pub fn new(jwt_secret: String, hmac_key: Vec<u8>) -> Self {
        Self {
            jwt_secret,
            hmac_keys: vec![hmac_key],
            active_kid: 0,
        }
    }

    pub fn generate_token(
        &self,
        user_id: &str,
        subscription_id: &str,
        tier: &str,
        rate_limit_bps: u64,
        expires_in_days: i64,
    ) -> Result<String, AuthError> {
        let now = Utc::now();
        let exp = now + Duration::days(expires_in_days);

        let claims = Claims {
            sub: subscription_id.to_string(),
            user_id: user_id.to_string(),
            tier: tier.to_string(),
            rate_limit_bps,
            exp: exp.timestamp(),
            iat: now.timestamp(),
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_ref()),
        )
        .map_err(|_| AuthError::AuthFailed)
    }

    pub fn validate_token(&self, token: &str) -> Result<Claims, AuthError> {
        let validation = Validation::default();
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_ref()),
            &validation,
        )
        .map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::TokenExpired,
            jsonwebtoken::errors::ErrorKind::InvalidSignature => AuthError::InvalidSignature,
            _ => AuthError::AuthFailed,
        })?;

        Ok(token_data.claims)
    }

    pub fn generate_hmac_signature(&self, data: &[u8]) -> Vec<u8> {
        let key = &self.hmac_keys[self.active_kid as usize];
        let mut mac = Hmac::<Sha256>::new_from_slice(key)
            .expect("HMAC can take key of any size");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    /// Constant-time comparison для HMAC (защита от timing side-channel).
    pub fn verify_hmac_signature(&self, data: &[u8], signature: &[u8]) -> bool {
        let expected = self.generate_hmac_signature(data);
        if expected.len() != signature.len() {
            return false;
        }
        expected.ct_eq(signature).into()
    }

    pub fn active_kid(&self) -> u8 {
        self.active_kid
    }

    /// Ротация HMAC ключа: добавляет новый ключ и делает его активным.
    /// Старый ключ остаётся для dual-validation window.
    pub fn rotate_hmac_key(&mut self, new_key: Vec<u8>) {
        self.active_kid = self.hmac_keys.len() as u8;
        self.hmac_keys.push(new_key);
    }

    /// Удаляет старый ключ (после завершения dual-validation window).
    pub fn remove_hmac_key(&mut self, kid: u8) {
        if (kid as usize) < self.hmac_keys.len() && kid != self.active_kid {
            self.hmac_keys.remove(kid as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_generation_and_validation() {
        let manager = AuthManager::new(
            "test_secret_key_123".to_string(),
            b"test_hmac_key".to_vec(),
        );

        let token = manager
            .generate_token("user123", "sub456", "pro", 3 * 1024 * 1024, 30)
            .unwrap();

        let claims = manager.validate_token(&token).unwrap();
        assert_eq!(claims.user_id, "user123");
        assert_eq!(claims.sub, "sub456");
        assert_eq!(claims.tier, "pro");
        assert_eq!(claims.rate_limit_bps, 3 * 1024 * 1024);
    }

    #[test]
    fn test_hmac_signature() {
        let manager = AuthManager::new(
            "test_secret".to_string(),
            b"test_hmac_key".to_vec(),
        );

        let data = b"test data";
        let signature = manager.generate_hmac_signature(data);
        assert!(manager.verify_hmac_signature(data, &signature));
    }

    #[test]
    fn test_hmac_rejects_tampered() {
        let manager = AuthManager::new(
            "test_secret".to_string(),
            b"test_hmac_key".to_vec(),
        );

        let data = b"test data";
        let mut signature = manager.generate_hmac_signature(data);
        signature[0] ^= 0xFF;
        assert!(!manager.verify_hmac_signature(data, &signature));
    }

    #[test]
    fn test_key_rotation() {
        let mut manager = AuthManager::new(
            "test_secret".to_string(),
            b"key1".to_vec(),
        );
        assert_eq!(manager.active_kid(), 0);

        manager.rotate_hmac_key(b"key2".to_vec());
        assert_eq!(manager.active_kid(), 1);

        let sig = manager.generate_hmac_signature(b"data");
        assert!(manager.verify_hmac_signature(b"data", &sig));
    }

    #[test]
    fn test_unified_error_message() {
        let err = AuthError::AuthFailed;
        assert_eq!(err.client_message(), "ERR");

        let err = AuthError::TokenExpired;
        assert_eq!(err.client_message(), "ERR");

        let err = AuthError::InvalidSignature;
        assert_eq!(err.client_message(), "ERR");
    }
}
