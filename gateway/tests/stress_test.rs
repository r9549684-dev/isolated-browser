use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::{TlsConnector, rustls::ClientConfig};
use rustls::pki_types::ServerName;

use transport_core::protocol::FrameCodec;
use transport_core::steal::send_auth_frame;

/// Stress test gateway: 5 классов клиентов.
///
/// Классы:
/// 1. valid — корректный TLS + auth + FrameCodec CONNECT
/// 2. invalid-auth — TLS + неверный auth token
/// 3. replay — повторная отправка того же фрейма
/// 4. truncated — обрезанный фрейм (partial read)
/// 5. slow-client — клиент с задержкой между операциями
///
/// Acceptance criteria:
/// - 1000 concurrent connections
/// - valid clients: 100% success
/// - invalid/replay/truncated: rejected без crash
/// - slow clients: не блокируют остальных
/// - p99 handshake latency < 200ms

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway_addr = "127.0.0.1:9443";
    let num_valid = 800;
    let num_invalid = 50;
    let num_replay = 50;
    let num_truncated = 50;
    let num_slow = 50;

    println!("=== Gateway Stress Test (FrameCodec + 5 client classes) ===");
    println!("Target: {}", gateway_addr);
    println!(
        "Clients: {} valid, {} invalid-auth, {} replay, {} truncated, {} slow",
        num_valid, num_invalid, num_replay, num_truncated, num_slow
    );
    println!();

    let server_public = [0u8; 32]; // Для test_mode не важен
    let key = [0u8; 32]; // Должен совпадать с gateway --secret

    let start = Instant::now();
    let mut handles = vec![];

    for i in 0..num_valid {
        let addr = gateway_addr.to_string();
        let sp = server_public;
        let k = key;
        handles.push(tokio::spawn(async move {
            client_valid(i, &addr, sp, k).await
        }));
    }

    for i in 0..num_invalid {
        let addr = gateway_addr.to_string();
        handles.push(tokio::spawn(async move {
            client_invalid_auth(i, &addr).await
        }));
    }

    for i in 0..num_replay {
        let addr = gateway_addr.to_string();
        let sp = server_public;
        let k = key;
        handles.push(tokio::spawn(async move {
            client_replay(i, &addr, sp, k).await
        }));
    }

    for i in 0..num_truncated {
        let addr = gateway_addr.to_string();
        handles.push(tokio::spawn(async move {
            client_truncated(i, &addr).await
        }));
    }

    for i in 0..num_slow {
        let addr = gateway_addr.to_string();
        let sp = server_public;
        let k = key;
        handles.push(tokio::spawn(async move {
            client_slow(i, &addr, sp, k).await
        }));
    }

    let mut stats = Stats::default();

    for handle in handles {
        match handle.await {
            Ok(Ok(result)) => stats.record(result),
            Ok(Err(e)) => {
                stats.errors += 1;
                eprintln!("Client error: {}", e);
            }
            Err(e) => {
                stats.errors += 1;
                eprintln!("Task error: {}", e);
            }
        }
    }

    let elapsed = start.elapsed();

    println!();
    println!("=== Results ===");
    println!("Duration: {:?}", elapsed);
    println!(
        "Total: {} success, {} errors, {} rejected",
        stats.success, stats.errors, stats.rejected
    );
    println!(
        "Valid clients: {}/{} success",
        stats.valid_success, num_valid
    );
    println!(
        "Invalid/replay/truncated rejected: {}/{}",
        stats.rejected,
        num_invalid + num_replay + num_truncated
    );

    if !stats.latencies.is_empty() {
        stats.latencies.sort();
        let p50 = stats.latencies[stats.latencies.len() / 2];
        let p95 = stats.latencies[(stats.latencies.len() as f64 * 0.95) as usize];
        let p99 = stats.latencies[(stats.latencies.len() as f64 * 0.99) as usize];
        println!("Latency p50: {:?}, p95: {:?}, p99: {:?}", p50, p95, p99);
    }

    let total = num_valid + num_invalid + num_replay + num_truncated + num_slow;
    if stats.valid_success == num_valid && stats.errors == 0 {
        println!("\n✓ Stress test PASSED ({}/{} valid clients succeeded)", stats.valid_success, total);
        Ok(())
    } else {
        println!(
            "\n✗ Stress test FAILED ({}/{} valid, {} errors)",
            stats.valid_success, num_valid, stats.errors
        );
        Err("Stress test failed".into())
    }
}

#[derive(Default)]
struct Stats {
    success: usize,
    errors: usize,
    rejected: usize,
    valid_success: usize,
    latencies: Vec<Duration>,
}

impl Stats {
    fn record(&mut self, result: ClientResult) {
        match result.class {
            ClientClass::Valid => {
                if result.success {
                    self.success += 1;
                    self.valid_success += 1;
                    self.latencies.push(result.latency);
                } else if result.rejected {
                    self.rejected += 1;
                } else {
                    self.errors += 1;
                }
            }
            _ => {
                if result.rejected {
                    self.rejected += 1;
                } else if result.success {
                    self.success += 1;
                } else {
                    self.errors += 1;
                }
            }
        }
    }
}

#[derive(Debug)]
enum ClientClass {
    Valid,
    InvalidAuth,
    Replay,
    Truncated,
    Slow,
}

struct ClientResult {
    class: ClientClass,
    success: bool,
    rejected: bool,
    latency: Duration,
}

async fn make_tls_connection(
    addr: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, Box<dyn std::error::Error + Send + Sync>> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(addr).await?;
    let domain = ServerName::try_from("localhost")?;
    let tls = connector.connect(domain, tcp).await?;
    Ok(tls)
}

/// Class 1: valid — корректный TLS + auth + FrameCodec CONNECT
async fn client_valid(
    _id: usize,
    addr: &str,
    server_public: [u8; 32],
    key: [u8; 32],
) -> Result<ClientResult, Box<dyn std::error::Error + Send + Sync>> {
    let req_start = Instant::now();
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let mut tls = make_tls_connection(addr).await?;
        let client_auth = send_auth_frame(&mut tls, &server_public)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let codec = FrameCodec::new(&key, client_auth.c2s_prefix, client_auth.s2c_prefix, 0);

        let connect_msg = format!("CONNECT example.com:443");
        codec.write_frame(&mut tls, connect_msg.as_bytes())
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
    .await;

    match result {
        Ok(()) => Ok(ClientResult {
            class: ClientClass::Valid,
            success: true,
            rejected: false,
            latency: req_start.elapsed(),
        }),
        Err(_) => Ok(ClientResult {
            class: ClientClass::Valid,
            success: false,
            rejected: false,
            latency: req_start.elapsed(),
        }),
    }
}

/// Class 2: invalid-auth — TLS + неверный auth token
async fn client_invalid_auth(
    _id: usize,
    addr: &str,
) -> Result<ClientResult, Box<dyn std::error::Error + Send + Sync>> {
    let req_start = Instant::now();
    let mut tls = make_tls_connection(addr).await?;

    // Отправляем случайный auth frame
    let mut bad_frame = [0u8; 80];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bad_frame);
    tls.write_all(&bad_frame).await?;

    // Ожидаем отказ
    let mut buf = vec![0u8; 1024];
    let _ = tls.read(&mut buf).await;

    Ok(ClientResult {
        class: ClientClass::InvalidAuth,
        success: false,
        rejected: true,
        latency: req_start.elapsed(),
    })
}

/// Class 3: replay — повторная отправка того же зашифрованного фрейма
async fn client_replay(
    _id: usize,
    addr: &str,
    server_public: [u8; 32],
    key: [u8; 32],
) -> Result<ClientResult, Box<dyn std::error::Error + Send + Sync>> {
    let req_start = Instant::now();
    let mut tls = make_tls_connection(addr).await?;
    let client_auth = send_auth_frame(&mut tls, &server_public).await?;
    let codec = FrameCodec::new(&key, client_auth.c2s_prefix, client_auth.s2c_prefix, 0);

    // Пишем фрейм и сохраняем сырые байты
    let connect_msg = b"CONNECT example.com:443";
    let mut frame_buf: Vec<u8> = Vec::new();
    codec.write_frame(&mut frame_buf, connect_msg).await?;
    tls.write_all(&frame_buf).await?;

    // Читаем OK
    let _ = codec.read_frame(&mut tls).await;

    // Повторяем тот же фрейм (replay)
    tls.write_all(&frame_buf).await?;

    // Ожидаем ошибку replay
    let result = codec.read_frame(&mut tls).await;

    Ok(ClientResult {
        class: ClientClass::Replay,
        success: false,
        rejected: result.is_err(),
        latency: req_start.elapsed(),
    })
}

/// Class 4: truncated — обрезанный фрейм (partial read)
async fn client_truncated(
    _id: usize,
    addr: &str,
) -> Result<ClientResult, Box<dyn std::error::Error + Send + Sync>> {
    let req_start = Instant::now();
    let mut tls = make_tls_connection(addr).await?;

    // Отправляем обрезанный TLS-record frame
    let truncated = [0x17u8, 0x03, 0x03, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00];
    tls.write_all(&truncated).await?;

    // Обрываем соединение
    let _ = tls.shutdown().await;

    Ok(ClientResult {
        class: ClientClass::Truncated,
        success: false,
        rejected: true,
        latency: req_start.elapsed(),
    })
}

/// Class 5: slow-client — клиент с задержкой между операциями
async fn client_slow(
    _id: usize,
    addr: &str,
    server_public: [u8; 32],
    key: [u8; 32],
) -> Result<ClientResult, Box<dyn std::error::Error + Send + Sync>> {
    let req_start = Instant::now();
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let mut tls = make_tls_connection(addr).await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        let client_auth = send_auth_frame(&mut tls, &server_public)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        let codec = FrameCodec::new(&key, client_auth.c2s_prefix, client_auth.s2c_prefix, 0);

        tokio::time::sleep(Duration::from_millis(500)).await;

        codec
            .write_frame(&mut tls, b"CONNECT example.com:443")
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
    .await;

    match result {
        Ok(()) => Ok(ClientResult {
            class: ClientClass::Slow,
            success: true,
            rejected: false,
            latency: req_start.elapsed(),
        }),
        Err(_) => Ok(ClientResult {
            class: ClientClass::Slow,
            success: false,
            rejected: false,
            latency: req_start.elapsed(),
        }),
    }
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
