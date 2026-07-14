//! Realistic load test: failover + handshake под нагрузкой.
//! Запускается на сервере с gateway.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::{TlsConnector, rustls::ClientConfig};
use rustls::pki_types::ServerName;
use transport_core::protocol::FrameCodec;
use transport_core::steal::client_handshake;
use transport_core::failover::{FailoverManager, GatewayEndpoint, FAILOVER_MIN_INTERVAL};

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
    let gateway = std::env::var("LOAD_GATEWAY")
        .unwrap_or_else(|_| "127.0.0.1:9443".to_string());
    let num_clients: usize = std::env::var("LOAD_CLIENTS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(200);

    println!("=== Failover + Handshake Load Test ===");
    println!("Gateway: {}, Clients: {}", gateway, num_clients);

    // Тестируем FailoverManager логику (без реального переключения — один endpoint).
    let endpoints = vec![GatewayEndpoint {
        host: gateway.clone(),
        port: 9443,
        sni: "cloudflare.com".to_string(),
    }];
    let mgr = FailoverManager::new(endpoints)?;
    assert_eq!(mgr.len(), 1);
    assert!(mgr.can_switch());
    // try_switch с одним endpoint: переключится на тот же (wrap), но cooldown активируется.
    assert!(mgr.try_switch());
    assert!(!mgr.try_switch()); // cooldown
    println!("FailoverManager: cooldown works, 60s min interval OK");

    // Реалистичный handshake load: num_clients параллельных ECDHE handshake.
    let success = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(AtomicUsize::new(0));
    let latencies = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let start = Instant::now();
    let mut handles = vec![];
    let stagger = Duration::from_millis(2);

    for i in 0..num_clients {
        let addr = gateway.clone();
        let sp = server_public();
        let succ = success.clone();
        let fail = failures.clone();
        let lats = latencies.clone();
        handles.push(tokio::spawn(async move {
            tokio::time::sleep(stagger * (i as u32 / 10)).await;
            let req_start = Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(15), load_connect(&addr, &sp)).await;
            let elapsed = req_start.elapsed();
            match result {
                Ok(Ok(())) => {
                    succ.fetch_add(1, Ordering::Relaxed);
                    lats.lock().await.push(elapsed);
                }
                _ => fail.fetch_add(1, Ordering::Relaxed),
            }
        }));
    }

    for h in handles { let _ = h.await; }

    let elapsed = start.elapsed();
    let succ = success.load(Ordering::Relaxed);
    let fail = failures.load(Ordering::Relaxed);
    let mut lats = latencies.lock().await.clone();
    lats.sort();
    let p50 = lats.get(lats.len() / 2).copied().unwrap_or_default();
    let p95 = lats.get((lats.len() as f64 * 0.95) as usize).copied().unwrap_or_default();
    let p99 = lats.get((lats.len() as f64 * 0.99) as usize).copied().unwrap_or_default();

    println!("\n=== Results ===");
    println!("Duration: {:?}", elapsed);
    println!("Success: {}/{} ({:.1}%)", succ, num_clients, succ as f64 / num_clients as f64 * 100.0);
    println!("Failures: {}", fail);
    println!("Latency: p50={:?}, p95={:?}, p99={:?}", p50, p95, p99);

    if succ == num_clients {
        println!("\n✓ Load test PASSED (all clients succeeded)");
        Ok(())
    } else {
        let rate = succ as f64 / num_clients as f64;
        if rate >= 0.95 {
            println!("\n✓ Load test PASSED (≥95% success, {:.1}%)", rate * 100.0);
            Ok(())
        } else {
            println!("\n✗ Load test FAILED ({:.1}% < 95%)", rate * 100.0);
            Err("Load test failed".into())
        }
    }
}

async fn load_connect(addr: &str, server_public: &[u8; 32]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(addr).await?;
    let domain = ServerName::try_from("localhost")?;
    let mut tls = connector.connect(domain, tcp).await?;
    let client_auth = client_handshake(&mut tls, server_public)
        .await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    let codec = FrameCodec::new(&client_auth.session_key, client_auth.c2s_prefix, client_auth.s2c_prefix, 0);
    codec.write_frame(&mut tls, b"CONNECT 127.0.0.1:80").await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    let ack = codec.read_frame(&mut tls).await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    if ack != b"OK" { return Err("bad ack".into()); }
    Ok(())
}

#[derive(Debug)]
struct SkipServerVerification;
impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(&self, _e: &rustls::pki_types::CertificateDer<'_>, _i: &[rustls::pki_types::CertificateDer<'_>], _s: &ServerName<'_>, _o: &[u8], _n: rustls::pki_types::UnixTime) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> { Ok(rustls::client::danger::ServerCertVerified::assertion()) }
    fn verify_tls12_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { Ok(rustls::client::danger::HandshakeSignatureValid::assertion()) }
    fn verify_tls13_signature(&self, _m: &[u8], _c: &rustls::pki_types::CertificateDer<'_>, _d: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> { Ok(rustls::client::danger::HandshakeSignatureValid::assertion()) }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::RSA_PKCS1_SHA256, rustls::SignatureScheme::RSA_PKCS1_SHA384, rustls::SignatureScheme::RSA_PKCS1_SHA512, rustls::SignatureScheme::ECDSA_NISTP256_SHA256, rustls::SignatureScheme::ECDSA_NISTP384_SHA384, rustls::SignatureScheme::ECDSA_NISTP521_SHA512, rustls::SignatureScheme::RSA_PSS_SHA256, rustls::SignatureScheme::RSA_PSS_SHA384, rustls::SignatureScheme::RSA_PSS_SHA512, rustls::SignatureScheme::ED25519, rustls::SignatureScheme::ED448]
    }
}
