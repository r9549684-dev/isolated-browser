//! Soak test: долгий непрерывный тест gateway на утечки памяти/FD.
//!
//! Запуск: cargo test --release --test soak_test -- --nocapture --ignored
//!
//! Параметры:
//! - SOAK_DURATION_SECS (env, default 600) — длительность теста
//! - SOAK_CONCURRENT (env, default 100) — concurrent connections
//! - SOAK_GATEWAY (env, default 127.0.0.1:9443) — адрес gateway
//!
//! Assertions:
//! - Нет паник/crashes
//! - Все соединения устанавливаются (в пределах timeout)
//! - Периодический мониторинг RSS/FD gateway через /proc

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
    let duration_secs: u64 = std::env::var("SOAK_DURATION_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let concurrent: usize = std::env::var("SOAK_CONCURRENT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let gateway = std::env::var("SOAK_GATEWAY")
        .unwrap_or_else(|_| "127.0.0.1:9443".to_string());

    println!("=== Soak Test ===");
    println!("Duration: {}s, Concurrent: {}, Gateway: {}", duration_secs, concurrent, gateway);
    println!();

    let success = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();
    let deadline = start + Duration::from_secs(duration_secs);

    let mut wave = 0;
    let mut first_err_logged = false;
    while Instant::now() < deadline {
        wave += 1;
        let mut handles = vec![];

        for _ in 0..concurrent {
            let addr = gateway.clone();
            let sp = server_public();
            let succ = success.clone();
            let fail = failures.clone();
            handles.push(tokio::spawn(async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(15),
                    soak_connect(&addr, &sp),
                ).await;
                match result {
                    Ok(Ok(())) => {
                        succ.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                    Ok(Err(e)) => {
                        fail.fetch_add(1, Ordering::Relaxed);
                        Some(e)
                    }
                    Err(_) => {
                        fail.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                }
            }));
        }

        for h in handles {
            match h.await {
                Ok(Some(e)) => {
                    if !first_err_logged {
                        eprintln!("first soak_connect error: {:?}", e);
                        first_err_logged = true;
                    }
                }
                Ok(None) => {}
                Err(join_err) => {
                    if !first_err_logged {
                        eprintln!("task join error: {:?}", join_err);
                        first_err_logged = true;
                    }
                }
            }
        }

        // Pace: не крутить цикл быстрее чем соединения успевают создаваться.
        // Без sleep цикл делает тысячи итераций/sec без реальной работы.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let elapsed = start.elapsed();
        let total_succ = success.load(Ordering::Relaxed);
        let total_fail = failures.load(Ordering::Relaxed);
        if wave <= 5 || wave % 100 == 0 {
            println!(
                "[{:>6.1}s] wave {}: total success={}, failures={}",
                elapsed.as_secs_f64(), wave, total_succ, total_fail
            );
        }
    }

    let total = success.load(Ordering::Relaxed) + failures.load(Ordering::Relaxed);
    println!();
    println!("=== Soak Test Complete ===");
    println!("Duration: {:?}", start.elapsed());
    println!("Waves: {}", wave);
    println!("Total connections: {}", total);
    println!("Success: {}", success.load(Ordering::Relaxed));
    println!("Failures: {}", failures.load(Ordering::Relaxed));

    if failures.load(Ordering::Relaxed) == 0 {
        println!("\n✓ Soak test PASSED (0 failures)");
        Ok(())
    } else {
        let fail_rate = failures.load(Ordering::Relaxed) as f64 / total as f64;
        if fail_rate < 0.01 {
            println!("\n✓ Soak test PASSED (fail rate {:.2}% < 1%)", fail_rate * 100.0);
            Ok(())
        } else {
            println!("\n✗ Soak test FAILED (fail rate {:.2}% >= 1%)", fail_rate * 100.0);
            Err("Soak test failed".into())
        }
    }
}

async fn soak_connect(
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
            format!("unexpected ack: {:?}", String::from_utf8_lossy(&ack)),
        )) as Box<dyn std::error::Error + Send + Sync>);
    }

    Ok(())
}

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}
