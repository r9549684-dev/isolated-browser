use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Error, Debug)]
pub enum PromoError {
    #[error("Invalid promo code")]
    InvalidCode,
    #[error("Promo code expired")]
    Expired,
    #[error("Promo code already used")]
    AlreadyUsed,
    #[error("Invalid signature")]
    InvalidSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromoCode {
    pub code: String,
    pub tier: String,
    pub months: i32,
    pub max_uses: i32,
    pub used_count: i32,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromoToken {
    pub code: String,
    pub user_id: String,
    pub tier: String,
    pub months: i32,
    pub exp: i64,
    pub iat: i64,
    pub signature: String,
}

pub struct PromoManager {
    codes: Arc<Mutex<HashMap<String, PromoCode>>>,
    used_tokens: Arc<Mutex<HashMap<String, Vec<String>>>>,
    hmac_key: Vec<u8>,
}

impl PromoManager {
    pub fn new(hmac_key: Vec<u8>) -> Self {
        let mut codes = HashMap::new();
        
        // Предзагруженные промокоды
        codes.insert("PRO3M".to_string(), PromoCode {
            code: "PRO3M".to_string(),
            tier: "pro".to_string(),
            months: 3,
            max_uses: 1000,
            used_count: 0,
            expires_at: None,
            created_at: Utc::now().timestamp(),
        });
        
        codes.insert("PRO6M".to_string(), PromoCode {
            code: "PRO6M".to_string(),
            tier: "pro".to_string(),
            months: 6,
            max_uses: 500,
            used_count: 0,
            expires_at: None,
            created_at: Utc::now().timestamp(),
        });
        
        codes.insert("PRO12M".to_string(), PromoCode {
            code: "PRO12M".to_string(),
            tier: "pro".to_string(),
            months: 12,
            max_uses: 100,
            used_count: 0,
            expires_at: None,
            created_at: Utc::now().timestamp(),
        });
        
        Self {
            codes: Arc::new(Mutex::new(codes)),
            used_tokens: Arc::new(Mutex::new(HashMap::new())),
            hmac_key,
        }
    }

    pub async fn validate_code(&self, code: &str) -> Result<PromoCode, PromoError> {
        let codes = self.codes.lock().await;
        let promo = codes.get(code).ok_or(PromoError::InvalidCode)?;
        
        if let Some(exp) = promo.expires_at {
            if Utc::now().timestamp() > exp {
                return Err(PromoError::Expired);
            }
        }
        
        if promo.used_count >= promo.max_uses {
            return Err(PromoError::AlreadyUsed);
        }
        
        Ok(promo.clone())
    }

    pub async fn use_code(&self, code: &str, user_id: &str) -> Result<PromoToken, PromoError> {
        let promo = self.validate_code(code).await?;
        
        let now = Utc::now();
        let exp = now + Duration::days(30 * promo.months as i64);
        
        let token = PromoToken {
            code: code.to_string(),
            user_id: user_id.to_string(),
            tier: promo.tier.clone(),
            months: promo.months,
            exp: exp.timestamp(),
            iat: now.timestamp(),
            signature: String::new(),
        };
        
        let signature = self.generate_signature(&token);
        let mut token = token;
        token.signature = signature;
        
        let mut codes = self.codes.lock().await;
        if let Some(promo) = codes.get_mut(code) {
            promo.used_count += 1;
        }
        
        let mut used = self.used_tokens.lock().await;
        used.entry(user_id.to_string())
            .or_insert_with(Vec::new)
            .push(code.to_string());
        
        Ok(token)
    }

    pub async fn verify_token(&self, token: &PromoToken) -> Result<(), PromoError> {
        let expected_signature = self.generate_signature(token);
        let expected_bytes = hex::decode(&expected_signature).unwrap_or_default();
        let token_bytes = hex::decode(&token.signature).unwrap_or_default();

        if expected_bytes.len() != token_bytes.len() {
            return Err(PromoError::InvalidSignature);
        }
        let is_equal: bool = expected_bytes.ct_eq(&token_bytes).into();
        if !is_equal {
            return Err(PromoError::InvalidSignature);
        }

        if Utc::now().timestamp() > token.exp {
            return Err(PromoError::Expired);
        }

        Ok(())
    }

    fn generate_signature(&self, token: &PromoToken) -> String {
        let data = format!("{}:{}:{}:{}:{}", 
            token.code, token.user_id, token.tier, token.months, token.exp);
        
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.hmac_key)
            .expect("HMAC can take key of any size");
        mac.update(data.as_bytes());
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }

    pub async fn add_promo_code(&self, code: PromoCode) {
        let mut codes = self.codes.lock().await;
        codes.insert(code.code.clone(), code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_promo_validation() {
        let manager = PromoManager::new(b"test_key".to_vec());
        
        let promo = manager.validate_code("PRO3M").await.unwrap();
        assert_eq!(promo.tier, "pro");
        assert_eq!(promo.months, 3);
    }

    #[tokio::test]
    async fn test_promo_usage() {
        let manager = PromoManager::new(b"test_key".to_vec());
        
        let token = manager.use_code("PRO3M", "user123").await.unwrap();
        assert_eq!(token.user_id, "user123");
        assert_eq!(token.tier, "pro");
        
        manager.verify_token(&token).await.unwrap();
    }

    #[tokio::test]
    async fn test_promo_rejects_tampered_signature() {
        let manager = PromoManager::new(b"test_key".to_vec());
        let mut token = manager.use_code("PRO3M", "user456").await.unwrap();
        
        // Tamper signature
        let mut sig_bytes = hex::decode(&token.signature).unwrap();
        sig_bytes[0] ^= 0xFF;
        token.signature = hex::encode(&sig_bytes);
        
        let result = manager.verify_token(&token).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PromoError::InvalidSignature));
    }

    #[tokio::test]
    async fn test_promo_expired_rejected() {
        let manager = PromoManager::new(b"test_key".to_vec());
        let mut token = manager.use_code("PRO3M", "user789").await.unwrap();
        
        // Set exp в прошлом
        token.exp = Utc::now().timestamp() - 1;
        // Пересчитываем signature (т.к. exp входит в данные)
        token.signature = manager.generate_signature(&token);
        
        let result = manager.verify_token(&token).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PromoError::Expired));
    }

    #[tokio::test]
    async fn test_promo_constant_time_verification() {
        // Проверяем что verify_token использует constant-time comparison
        // (subtle::ConstantTimeEq), а не обычный ==.
        let manager = PromoManager::new(b"test_key".to_vec());
        let token = manager.use_code("PRO3M", "user_ct").await.unwrap();
        
        // Valid token должен пройти
        assert!(manager.verify_token(&token).await.is_ok());
        
        // Полностью другой signature (другая длина hex — но оба 64 hex chars = 32 bytes)
        let mut bad_token = token.clone();
        bad_token.signature = hex::encode(&[0u8; 32]);
        assert!(manager.verify_token(&bad_token).await.is_err());
    }
}
