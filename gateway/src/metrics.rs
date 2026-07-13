//! Prometheus metrics для gateway.
//!
//! Exposition format: text-based, отдаётся на /metrics endpoint.
//! Метрики:
//! - gateway_connections_active (gauge)
//! - gateway_connections_total (counter)
//! - gateway_auth_failures_total{type} (counter)
//! - gateway_handshake_latency_seconds (histogram)
//! - gateway_rekey_total (counter)
//! - gateway_replay_detected_total (counter)

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use once_cell::sync::Lazy;

struct Metrics {
    connections_active: AtomicI64,
    connections_total: AtomicU64,
    auth_failures_invalid: AtomicU64,
    auth_failures_replay: AtomicU64,
    auth_failures_truncated: AtomicU64,
    rekey_total: AtomicU64,
    replay_detected_total: AtomicU64,
    handshake_latency_samples: Mutex<Vec<f64>>,
}

use std::sync::atomic::AtomicI64;

static METRICS: Lazy<Metrics> = Lazy::new(|| Metrics {
    connections_active: AtomicI64::new(0),
    connections_total: AtomicU64::new(0),
    auth_failures_invalid: AtomicU64::new(0),
    auth_failures_replay: AtomicU64::new(0),
    auth_failures_truncated: AtomicU64::new(0),
    rekey_total: AtomicU64::new(0),
    replay_detected_total: AtomicU64::new(0),
    handshake_latency_samples: Mutex::new(Vec::new()),
});

pub fn inc_connections_active() {
    METRICS.connections_active.fetch_add(1, Ordering::Relaxed);
}

pub fn dec_connections_active() {
    METRICS.connections_active.fetch_sub(1, Ordering::Relaxed);
}

pub fn inc_connections_total() {
    METRICS.connections_total.fetch_add(1, Ordering::Relaxed);
}

pub fn inc_auth_failure(kind: &str) {
    match kind {
        "invalid" => METRICS.auth_failures_invalid.fetch_add(1, Ordering::Relaxed),
        "replay" => METRICS.auth_failures_replay.fetch_add(1, Ordering::Relaxed),
        "truncated" => METRICS.auth_failures_truncated.fetch_add(1, Ordering::Relaxed),
        _ => return,
    };
}

pub fn inc_rekey() {
    METRICS.rekey_total.fetch_add(1, Ordering::Relaxed);
}

pub fn inc_replay_detected() {
    METRICS.replay_detected_total.fetch_add(1, Ordering::Relaxed);
}

pub fn record_handshake_latency(secs: f64) {
    if let Ok(mut samples) = METRICS.handshake_latency_samples.lock() {
        samples.push(secs);
        if samples.len() > 10000 {
            samples.drain(0..5000);
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64) * p) as usize;
    let idx = idx.min(sorted.len() - 1);
    sorted[idx]
}

/// Возвращает Prometheus exposition format text.
pub fn render() -> String {
    let active = METRICS.connections_active.load(Ordering::Relaxed);
    let total = METRICS.connections_total.load(Ordering::Relaxed);
    let fail_invalid = METRICS.auth_failures_invalid.load(Ordering::Relaxed);
    let fail_replay = METRICS.auth_failures_replay.load(Ordering::Relaxed);
    let fail_truncated = METRICS.auth_failures_truncated.load(Ordering::Relaxed);
    let rekey = METRICS.rekey_total.load(Ordering::Relaxed);
    let replay = METRICS.replay_detected_total.load(Ordering::Relaxed);

    let (p50, p95, p99, count) = {
        if let Ok(samples) = METRICS.handshake_latency_samples.lock() {
            if samples.is_empty() {
                (0.0, 0.0, 0.0, 0u64)
            } else {
                let mut sorted = samples.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                (
                    percentile(&sorted, 0.50),
                    percentile(&sorted, 0.95),
                    percentile(&sorted, 0.99),
                    sorted.len() as u64,
                )
            }
        } else {
            (0.0, 0.0, 0.0, 0u64)
        }
    };

    format!(
        "# HELP gateway_connections_active Currently active connections\n\
         # TYPE gateway_connections_active gauge\n\
         gateway_connections_active {}\n\
         # HELP gateway_connections_total Total connections accepted\n\
         # TYPE gateway_connections_total counter\n\
         gateway_connections_total {}\n\
         # HELP gateway_auth_failures_total Authentication failures by type\n\
         # TYPE gateway_auth_failures_total counter\n\
         gateway_auth_failures_total{{type=\"invalid\"}} {}\n\
         gateway_auth_failures_total{{type=\"replay\"}} {}\n\
         gateway_auth_failures_total{{type=\"truncated\"}} {}\n\
         # HELP gateway_rekey_total Total rekey operations\n\
         # TYPE gateway_rekey_total counter\n\
         gateway_rekey_total {}\n\
         # HELP gateway_replay_detected_total Replay attacks detected\n\
         # TYPE gateway_replay_detected_total counter\n\
         gateway_replay_detected_total {}\n\
         # HELP gateway_handshake_latency_seconds Handshake latency distribution\n\
         # TYPE gateway_handshake_latency_seconds summary\n\
         gateway_handshake_latency_seconds{{quantile=\"0.5\"}} {}\n\
         gateway_handshake_latency_seconds{{quantile=\"0.95\"}} {}\n\
         gateway_handshake_latency_seconds{{quantile=\"0.99\"}} {}\n\
         gateway_handshake_latency_seconds_count {}\n",
        active, total,
        fail_invalid, fail_replay, fail_truncated,
        rekey, replay,
        p50, p95, p99, count
    )
}
