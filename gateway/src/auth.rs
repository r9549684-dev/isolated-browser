use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Invalid token: {0}")]
    InvalidToken(String),
    #[error("Token expired")]
    TokenExpired,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Missing claims")]
    MissingClaims,
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

pub struct AuthManager {
    jwt_secret: String,
    hmac_key: Vec<u8>,
}

impl AuthManager {
    pub fn new(jwt_secret: String, hmac_key: Vec<u8>) -> Self {
        Self { jwt_secret, hmac_key }
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
        .map_err(|e| AuthError::InvalidToken(e.to_string()))
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
            _ => AuthError::InvalidToken(e.to_string()),
        })?;

        Ok(token_data.claims)
    }

    pub fn generate_hmac_signature(&self, data: &[u8]) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.hmac_key)
            .expect("HMAC can take key of any size");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    pub fn verify_hmac_signature(&self, data: &[u8], signature: &[u8]) -> bool {
        let expected = self.generate_hmac_signature(data);
        expected == signature
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
}
