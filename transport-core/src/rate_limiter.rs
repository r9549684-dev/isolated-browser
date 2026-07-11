use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::time::sleep;

pub struct TokenBucket {
    capacity: AtomicU64,
    tokens: AtomicU64,
    refill_rate: AtomicU64,
    last_refill: std::sync::Mutex<Instant>,
}

impl TokenBucket {
    pub fn new(capacity: u64, refill_rate: u64) -> Self {
        Self {
            capacity: AtomicU64::new(capacity),
            tokens: AtomicU64::new(capacity),
            refill_rate: AtomicU64::new(refill_rate),
            last_refill: std::sync::Mutex::new(Instant::now()),
        }
    }

    pub fn try_consume(&self, amount: u64) -> bool {
        self.refill();
        
        // CAS loop для атомарности
        loop {
            let current = self.tokens.load(Ordering::Acquire);
            if current < amount {
                return false;
            }
            
            // Атомарная операция compare_exchange
            match self.tokens.compare_exchange_weak(
                current,
                current - amount,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue, // Retry если другой поток изменил
            }
        }
    }

    pub async fn wait_for_tokens(&self, amount: u64) {
        loop {
            if self.try_consume(amount) {
                return;
            }
            // Async sleep вместо blocking
            sleep(Duration::from_millis(10)).await;
        }
    }

    fn refill(&self) {
        let mut last_refill = self.last_refill.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(*last_refill);
        
        if elapsed >= Duration::from_secs(1) {
            let refill_rate = self.refill_rate.load(Ordering::Acquire);
            let tokens_to_add = refill_rate * (elapsed.as_secs() as u64);
            
            // CAS loop для refill
            loop {
                let current = self.tokens.load(Ordering::Acquire);
                let capacity = self.capacity.load(Ordering::Acquire);
                let new_tokens = (current + tokens_to_add).min(capacity);
                
                match self.tokens.compare_exchange_weak(
                    current,
                    new_tokens,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(_) => continue,
                }
            }
            *last_refill = now;
        }
    }

    pub fn update_limits(&self, new_capacity: u64, new_refill_rate: u64) {
        self.capacity.store(new_capacity, Ordering::Release);
        self.refill_rate.store(new_refill_rate, Ordering::Release);
        self.tokens.store(new_capacity, Ordering::Release);
    }
}

pub struct RateLimiter {
    bucket: Arc<TokenBucket>,
}

impl RateLimiter {
    pub fn new(bytes_per_second: u64) -> Self {
        let capacity = bytes_per_second;
        let refill_rate = bytes_per_second;
        
        Self {
            bucket: Arc::new(TokenBucket::new(capacity, refill_rate)),
        }
    }

    pub fn try_consume(&self, bytes: u64) -> bool {
        self.bucket.try_consume(bytes)
    }

    pub fn wait_for_bytes(&self, bytes: u64) {
        loop {
            if self.bucket.try_consume(bytes) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub async fn wait_for_bytes_async(&self, bytes: u64) {
        self.bucket.wait_for_tokens(bytes).await;
    }

    pub fn update_limit(&self, new_bytes_per_second: u64) {
        self.bucket.update_limits(new_bytes_per_second, new_bytes_per_second);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_basic() {
        let bucket = TokenBucket::new(1000, 1000);
        assert!(bucket.try_consume(500));
        assert!(bucket.try_consume(500));
        assert!(!bucket.try_consume(100));
    }

    #[test]
    fn test_rate_limiter() {
        let limiter = RateLimiter::new(1000);
        assert!(limiter.try_consume(500));
        assert!(limiter.try_consume(500));
        assert!(!limiter.try_consume(100));
    }

    #[test]
    fn test_update_limit() {
        let limiter = RateLimiter::new(1000);
        assert!(!limiter.try_consume(1500));
        
        limiter.update_limit(2000);
        assert!(limiter.try_consume(1500));
    }
}
