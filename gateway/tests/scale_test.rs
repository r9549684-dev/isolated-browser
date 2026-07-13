//! Scale test: пошаговое увеличение concurrent connections для определения пределов.
//!
//! Запуск: cargo test --release --test scale_test -- --nocapture
//!
//! Ступени: 1000, 2000, 3000, 4000, 5000 concurrent.
//! На каждой ступени:成功率, latency p99, errors.
//! Цель: определить max concurrent для 2-core VPS.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::{TlsConnector, rustls::ClientConfig};
use rustls::pki_types::ServerName;

use transport_core::protocol::FrameCodec;
use transport_core::steal::client_handshake;

const TEST_MODE_SERVER_SECRET: [u8; 32] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
];

fn server_public() -> [u8; 32] {
    use x25519_dalek::{PublicKey, StaticSecret};
    let secret = StaticSecret::from(TEST_MODE_SERVER_SECRET);
    *PublicKey::from(&secret).as_bytes()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway = std::env::var("SCALE_GATEWAY")
        .unwrap_or_else(|_| "127.0.0.1:9443".to_string());
    let stages: Vec<usize> = std::env::var("SCALE_STAGES")
        .ok()
        .map(|s| s.split(',').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_else(|| vec![1000, 2000, 3000, 4000, 5000]);

    println!("=== Scale Test (best-effort 2-core VPS) ===");
    println!("Gateway: {}", gateway);
    println!("Stages: {:?}", stages);
    println!();

    let mut results = Vec::new();

    for &concurrent in &stages {
        println!("--- Stage: {} concurrent ---", concurrent);
        let success = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(AtomicUsize::new(0));
        let latencies = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let start = Instant::now();
        let mut handles = vec![];
        let stagger = Duration::from_millis(1);

        for i in 0..concurrent {
            let addr = gateway.clone();
            let sp = server_public();
            let succ = success.clone();
            let fail = failures.clone();
            let lats = latencies.clone();
            handles.push(tokio::spawn(async move {
                tokio::time::sleep(stagger * (i as u32 / 20)).await;
                let req_start = Instant::now();
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    scale_connect(&addr, &sp),
                ).await;
                let elapsed = req_start.elapsed();
                match result {
                    Ok(Ok(())) => {
                        succ.fetch_add(1, Ordering::Relaxed);
                        lats.lock().await.push(elapsed);
                    }
                    Ok(Err(_e)) => {
                        fail.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        fail.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }));
        }

        for h in handles {
            let _ = h.await;
        }

        let elapsed = start.elapsed();
        let succ = success.load(Ordering::Relaxed);
        let fail = failures.load(Ordering::Relaxed);
        let mut lats = latencies.lock().await.clone();
        lats.sort();

        let p50 = if !lats.is_empty() { lats[lats.len() / 2] } else { Duration::from_secs(0) };
        let p95 = if !lats.is_empty() { lats[(lats.len() as f64 * 0.95) as usize] } else { Duration::from_secs(0) };
        let p99 = if !lats.is_empty() { lats[(lats.len() as f64 * 0.99) as usize] } else { Duration::from_secs(0) };

        let success_rate = succ as f64 / concurrent as f64 * 100.0;
        println!(
            "  result: {}/{} success ({:.1}%), {} failures, {:?}",
            succ, concurrent, success_rate, fail, elapsed
        );
        println!("  latency: p50={:?}, p95={:?}, p99={:?}", p50, p95, p99);

        results.push((concurrent, succ, fail, p99, success_rate));

        // Пауза между ступенями.
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    println!();
    println!("=== Scale Test Summary ===");
    println!("{:<10} {:<10} {:<10} {:<12} {:<10}", "Concurrent", "Success", "Failures", "p99", "Success%");
    for (c, s, f, p99, sr) in &results {
        println!("{:<10} {:<10} {:<10} {:<12} {:<.1}%", c, s, f, format!("{:?}", p99), sr);
    }

    // Определить max concurrent с success rate >= 95%.
    let max_ok = results.iter()
        .filter(|(_, _, _, _, sr)| *sr >= 95.0)
        .map(|(c, _, _, _, _)| *c)
        .max();
    if let Some(max) = max_ok {
        println!("\nMax concurrent (>=95% success): {}", max);
    } else {
        println!("\nNo stage reached 95% success rate");
    }

    Ok(())
}

async fn scale_connect(
    addr: &str,
    server_public: &[u8; 32],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(addr).await?;
    let domain = ServerName::try_from("localhost")?;
    let mut tls = connector.connect(domain, tcp).await?;

    let client_auth = client_handshake(&mut tls, server_public)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    let codec = FrameCodec::new(&client_auth.session_key, client_auth.c2s_prefix, client_auth.s2c_prefix, 0);

    codec.write_frame(&mut tls, b"CONNECT 127.0.0.1:80")
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    let ack = codec.read_frame(&mut tls)
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    if ack != b"OK" {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "unexpected ack",
        )) as Box<dyn std::error::Error + Send + Sync>);
    }
    Ok(())
}

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self, _e: &rustls::pki_types::CertificateDer<'_>, _i: &[rustls::pki_types::CertificateDer<'_>],
        _s: &ServerName<'_>, _o: &[u8], _n: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256, rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512, rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384, rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256, rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512, rustls::SignatureScheme::ED25519, rustls::SignatureScheme::ED448,
        ]
    }
}
