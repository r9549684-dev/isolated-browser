/// Сервер-шлюз (gateway) с TCP-level обработкой для защиты от active probing.
///
/// Архитектура (как в REALITY):
/// 1. Принять TCP соединение
/// 2. Прочитать ClientHello (первые байты TLS handshake)
/// 3. Извлечь auth token из ClientHello (из session_id)
/// 4. Если auth валиден — продолжить TLS handshake через rustls
/// 5. Если auth невалиден — проксировать сырые TCP байты на реальный CDN (fallback)
///
/// Защита:
/// - Per-IP rate limiting на этапе handshake (до auth)
/// - Bounded queues + idle timeout (300s)
/// - Frame/body size limits (16KB payload, 64KB max frame body)
/// - Constant-time HMAC + unified error responses (anti timing oracle)
/// - Replay protection (sliding window)
/// - Key versioning (kid field, 2 active keys)

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::fs;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, Ordering};

use clap::Parser;
use rustls::ServerConfig;
use rustls_pemfile::{certs, private_key};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};

use transport_core::protocol::FrameCodec;
use transport_core::steal::{read_auth_frame, server_derive_session, send_rekey_init, server_handle_rekey_ack};
use transport_core::tcp_handler::{extract_auth_from_clienthello, fallback_tcp_proxy};
use transport_core::error::TransportError;

mod auth;
mod promo;
mod metrics;
use auth::{AuthManager, Claims};

const MAX_CONCURRENT_CONNECTIONS: usize = 5000;
const IDLE_TIMEOUT_SECS: u64 = 300;

/// Фиксированный X25519 static secret для test_mode.
/// Stress_test знает этот secret и вычисляет соответствующий public key
/// через x25519_dalek для ECDHE handshake.
pub const TEST_MODE_SERVER_SECRET: [u8; 32] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
    0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
];

/// Per-IP rate limiter для защиты от handshake flood (до auth).
/// Sliding window: max N connections per IP per WINDOW_SECS.
const IP_RATE_LIMIT_WINDOW_SECS: u64 = 60;
const IP_RATE_LIMIT_MAX_CONNS: usize = 20;
/// Максимум отслеживаемых IP-адресов (защита от memory exhaustion).
/// При превышении — LRU eviction самых старых записей.
const IP_RATE_LIMIT_MAX_ENTRIES: usize = 10000;

struct IpRateLimiter {
    connections: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl IpRateLimiter {
    fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    /// Проверяет, может ли IP установить новое соединение.
    /// Возвращает true если разрешено, false если превышен лимит.
    /// При превышении MAX_ENTRIES — удаляет самые старые записи (LRU).
    async fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let window = Duration::from_secs(IP_RATE_LIMIT_WINDOW_SECS);

        let mut conns = self.connections.lock().await;

        // LRU eviction: если слишком много записей, удаляем самые старые
        if conns.len() > IP_RATE_LIMIT_MAX_ENTRIES {
            // Собираем (ip, oldest_timestamp) и сортируем
            let mut ip_oldest: Vec<(IpAddr, Instant)> = conns
                .iter()
                .filter_map(|(ip, ts)| ts.first().map(|t| (*ip, *t)))
                .collect();
            ip_oldest.sort_by_key(|(_, t)| *t);
            // Удаляем 20% самых старых
            let to_remove = ip_oldest.len() / 5;
            for (ip, _) in ip_oldest.into_iter().take(to_remove) {
                conns.remove(&ip);
            }
        }

        let entry = conns.entry(ip).or_insert_with(Vec::new);
        entry.retain(|t| now.duration_since(*t) < window);

        if entry.len() >= IP_RATE_LIMIT_MAX_CONNS {
            false
        } else {
            entry.push(now);
            true
        }
    }

    /// Периодическая очистка устаревших записей.
    async fn cleanup(&self) {
        let now = Instant::now();
        let window = Duration::from_secs(IP_RATE_LIMIT_WINDOW_SECS);
        let mut conns = self.connections.lock().await;
        conns.retain(|_, timestamps| {
            timestamps.retain(|t| now.duration_since(*t) < window);
            !timestamps.is_empty()
        });
    }
}

/// Rate limiter для серверной части (token bucket)
struct ServerRateLimiter {
    capacity: AtomicU64,
    tokens: AtomicU64,
    refill_rate: AtomicU64,
    last_refill: Mutex<Instant>,
}

impl ServerRateLimiter {
    fn new(bytes_per_second: u64) -> Self {
        Self {
            capacity: AtomicU64::new(bytes_per_second),
            tokens: AtomicU64::new(bytes_per_second),
            refill_rate: AtomicU64::new(bytes_per_second),
            last_refill: Mutex::new(Instant::now()),
        }
    }

    async fn try_consume(&self, amount: u64) -> bool {
        self.refill().await;

        loop {
            let current = self.tokens.load(Ordering::Acquire);
            if current < amount {
                return false;
            }
            match self.tokens.compare_exchange_weak(
                current,
                current - amount,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }

    async fn refill(&self) {
        let mut last_refill = self.last_refill.lock().await;
        let now = Instant::now();
        let elapsed = now.duration_since(*last_refill);

        if elapsed >= Duration::from_secs(1) {
            let tokens_to_add =
                self.refill_rate.load(Ordering::Acquire) * (elapsed.as_secs() as u64);
            let capacity = self.capacity.load(Ordering::Acquire);
            loop {
                let current = self.tokens.load(Ordering::Acquire);
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
}

/// Менеджер подписок с JWT аутентификацией
struct SubscriptionManager {
    auth_manager: Arc<Mutex<AuthManager>>,
}

impl SubscriptionManager {
    fn new(jwt_secret: String, hmac_key: Vec<u8>) -> Self {
        Self {
            auth_manager: Arc::new(Mutex::new(AuthManager::new(jwt_secret, hmac_key))),
        }
    }

    async fn validate_subscription_token(&self, token: &str) -> Option<Claims> {
        let mgr = self.auth_manager.lock().await;
        match mgr.validate_token(token) {
            Ok(claims) => Some(claims),
            Err(e) => {
                warn!("token validation failed: {:?}", e);
                None
            }
        }
    }

    async fn get_rate_limit_from_claims(&self, claims: &Claims) -> u64 {
        claims.rate_limit_bps
    }
}

#[derive(Parser)]
#[command(name = "gateway", about = "Isolated Browser Gateway Server (TCP-level steal-TLS)")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:443")]
    bind: SocketAddr,

    #[arg(long, help = "Path to TLS certificate PEM")]
    cert: String,

    #[arg(long, help = "Path to TLS private key PEM")]
    key: String,

    #[arg(long, help = "Hex-encoded 32-byte shared secret for ChaCha20-Poly1305")]
    secret: String,

    #[arg(long, help = "Hex-encoded 32-byte X25519 server private key")]
    server_private_key: String,

    #[arg(long, default_value = "cloudflare.com:443", help = "Fallback CDN for stealth mode")]
    fallback_cdn: String,

    #[arg(long, env = "JWT_SECRET", help = "JWT secret for subscription tokens")]
    jwt_secret: String,

    #[arg(long, env = "HMAC_KEY", help = "Hex-encoded HMAC key for data signing")]
    hmac_key: String,

    #[arg(long, default_value = "0.0.0.0:9090", help = "Prometheus metrics endpoint")]
    metrics_bind: SocketAddr,

    #[arg(long, help = "Test mode: accept any subscription token")]
    test_mode: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "gateway=info".into()),
        )
        .init();

    let args = Args::parse();
    let test_mode = args.test_mode;

    let _key = if test_mode {
        info!("test_mode: PFS active — session keys derived via ECDHE");
        [0u8; 32]
    } else {
        parse_hex_key(&args.secret)?
    };
    // В test_mode используем фиксированный server_private_key чтобы stress_test
    // мог использовать соответствующий public key. Реальная безопасность не нужна.
    let server_private_key = if test_mode {
        info!("test_mode: using fixed server_private_key for stress test compatibility");
        TEST_MODE_SERVER_SECRET
    } else {
        parse_hex_key(&args.server_private_key)?
    };
    let fallback_cdn = args.fallback_cdn.clone();
    let hmac_key = parse_hex_key(&args.hmac_key)?;

    let tls_config = load_tls_config(&args.cert, &args.key)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let subscription_manager = Arc::new(SubscriptionManager::new(
        args.jwt_secret.clone(),
        hmac_key.to_vec(),
    ));
    let connection_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    let ip_rate_limiter = Arc::new(IpRateLimiter::new());

    let listener = TcpListener::bind(args.bind).await?;
    info!(
        "gateway listening on {} (max_connections: {}, ip_rate_limit: {}/{:?})",
        args.bind, MAX_CONCURRENT_CONNECTIONS, IP_RATE_LIMIT_MAX_CONNS, IP_RATE_LIMIT_WINDOW_SECS
    );

    let cleanup_limiter = ip_rate_limiter.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            cleanup_limiter.cleanup().await;
        }
    });

    // Prometheus metrics endpoint
    let metrics_addr = args.metrics_bind;
    tokio::spawn(async move {
        let listener = TcpListener::bind(metrics_addr).await.expect("metrics bind failed");
        info!("metrics endpoint listening on {}", metrics_addr);
        loop {
            if let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let body = metrics::render();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        }
    });

    loop {
        let (tcp, peer) = listener.accept().await?;

        // Per-IP rate limiting (до auth, до semaphore)
        // В test_mode пропускаем rate limiting для loopback
        if !test_mode || !peer.ip().is_loopback() {
            if !ip_rate_limiter.check(peer.ip()).await {
                warn!("IP rate limit exceeded for {}, rejecting", peer.ip());
                drop(tcp);
                continue;
            }
        }

        let permit = match connection_semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                warn!(
                    "connection limit reached ({}), rejecting {}",
                    MAX_CONCURRENT_CONNECTIONS, peer
                );
                drop(tcp);
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let fallback = fallback_cdn.clone();
        let sub_mgr = subscription_manager.clone();
        let test = test_mode;
        metrics::inc_connections_active();
        metrics::inc_connections_total();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) =
                handle_tcp_connection(tcp, server_private_key, fallback, acceptor, sub_mgr, test).await
            {
                debug!("client {} error: {:?}", peer, e);
            }
            metrics::dec_connections_active();
        });
    }
}

async fn handle_tcp_connection(
    mut tcp: TcpStream,
    server_private_key: [u8; 32],
    fallback_cdn: String,
    acceptor: TlsAcceptor,
    subscription_manager: Arc<SubscriptionManager>,
    test_mode: bool,
) -> Result<(), TransportError> {
    let mut buffer = vec![0u8; 16384];
    let n = tcp.read(&mut buffer).await.map_err(TransportError::Io)?;

    if n == 0 {
        return Err(TransportError::Protocol("empty connection".into()));
    }

    let client_hello_data = buffer[..n].to_vec();
    let auth_result = extract_auth_from_clienthello(&client_hello_data);

    match auth_result {
        Some((auth_token, _)) => {
            let ephemeral_public: [u8; 32] = auth_token;
            debug!("pre-TLS: auth token present in ClientHello — continuing TLS handshake");

            let prefixed_stream = PrefixedStream::new(tcp, client_hello_data);

            match acceptor.accept(prefixed_stream).await {
                Ok(mut tls_stream) => {
                    let (ephemeral_public, auth_token, c2s_prefix, s2c_prefix) =
                        read_auth_frame(&mut tls_stream).await?;

                    match server_derive_session(&server_private_key, &ephemeral_public, &auth_token) {
                        Some((server_ephemeral_public, session_key)) => {
                            tls_stream.write_all(&server_ephemeral_public).await?;
                            tls_stream.flush().await?;

                            let jwt_token = hex::encode(&ephemeral_public[..8]);

                            let claims = if test_mode {
                                Some(auth::Claims {
                                    sub: jwt_token.clone(),
                                    user_id: "test_user".to_string(),
                                    tier: "pro".to_string(),
                                    rate_limit_bps: 3 * 1024 * 1024,
                                    exp: chrono::Utc::now().timestamp() + 86400,
                                    iat: chrono::Utc::now().timestamp(),
                                })
                            } else {
                                subscription_manager.validate_subscription_token(&jwt_token).await
                            };

                            match claims {
                                Some(claims) => {
                                    let codec = FrameCodec::new(&session_key, s2c_prefix, c2s_prefix, 0);
                                    handle_authenticated_client(tls_stream, codec, subscription_manager, claims)
                                        .await?;
                                }
                                None => {
                                    send_unified_error_tls(&mut tls_stream).await;
                                    return Err(TransportError::AuthFailed);
                                }
                            }
                        }
                        None => {
                            send_unified_error_tls(&mut tls_stream).await;
                            metrics::inc_auth_failure("invalid");
                            return Err(TransportError::AuthFailed);
                        }
                    }
                }
                Err(e) => {
                    debug!("TLS accept error: {}", e);
                    return Err(TransportError::Protocol(format!("TLS accept failed: {}", e)));
                }
            }
        }
        None => {
            if test_mode {
                debug!("test_mode: no auth in ClientHello — continuing TLS handshake");
                let prefixed_stream = PrefixedStream::new(tcp, client_hello_data);

                match acceptor.accept(prefixed_stream).await {
                    Ok(mut tls_stream) => {
                        let (ephemeral_public, auth_token, c2s_prefix, s2c_prefix) =
                            read_auth_frame(&mut tls_stream).await?;

                        match server_derive_session(&server_private_key, &ephemeral_public, &auth_token) {
                            Some((server_ephemeral_public, session_key)) => {
                                tls_stream.write_all(&server_ephemeral_public).await?;
                                tls_stream.flush().await?;

                                let claims = auth::Claims {
                                    sub: "test".to_string(),
                                    user_id: "test_user".to_string(),
                                    tier: "pro".to_string(),
                                    rate_limit_bps: 3 * 1024 * 1024,
                                    exp: chrono::Utc::now().timestamp() + 86400,
                                    iat: chrono::Utc::now().timestamp(),
                                };

                                let codec = FrameCodec::new(&session_key, s2c_prefix, c2s_prefix, 0);
                                handle_authenticated_client(tls_stream, codec, subscription_manager, claims)
                                    .await?;
                            }
                            None => {
                                send_unified_error_tls(&mut tls_stream).await;
                                metrics::inc_auth_failure("invalid");
                                return Err(TransportError::AuthFailed);
                            }
                        }
                    }
                    Err(e) => {
                        debug!("TLS accept error: {}", e);
                        return Err(TransportError::Protocol(format!("TLS accept failed: {}", e)));
                    }
                }
            } else {
                debug!("no auth token — falling back to CDN");
                let _ = fallback_tcp_proxy(tcp, &fallback_cdn, &client_hello_data).await;
            }
        }
    }

    Ok(())
}

/// Отправляет унифицированную ошибку через TLS stream.
async fn send_unified_error_tls(tls: &mut tokio_rustls::server::TlsStream<PrefixedStream>) {
    let _ = tls.write_all(b"ERR").await;
    let _ = tls.flush().await;
}

async fn handle_authenticated_client(
    mut stream: tokio_rustls::server::TlsStream<PrefixedStream>,
    codec: FrameCodec,
    subscription_manager: Arc<SubscriptionManager>,
    claims: Claims,
) -> Result<(), TransportError> {
    // Читаем CONNECT frame
    let frame = codec.read_frame(&mut stream).await?;
    let cmd = String::from_utf8(frame)
        .map_err(|_| TransportError::Protocol("invalid CONNECT frame".into()))?;

    if !cmd.starts_with("CONNECT ") {
        codec.write_frame(&mut stream, b"ERR").await?;
        return Err(TransportError::Protocol(format!("unexpected command: {}", cmd)));
    }

    let target = cmd.trim_start_matches("CONNECT ").trim();
    debug!("→ {} (subscription: {}, tier: {})", target, claims.sub, claims.tier);

    let target_stream = match TcpStream::connect(target).await {
        Ok(s) => s,
        Err(e) => {
            codec.write_frame(&mut stream, b"ERR").await?;
            return Err(TransportError::Io(e));
        }
    };

    codec.write_frame(&mut stream, b"OK").await?;

    let rate_limit = subscription_manager.get_rate_limit_from_claims(&claims).await;
    let rate_limiter = Arc::new(ServerRateLimiter::new(rate_limit));

    debug!("subscription {} rate limit: {} bytes/sec", claims.sub, rate_limit);

    relay(stream, target_stream, codec, rate_limiter).await
}

struct PrefixedStream {
    inner: TcpStream,
    prefix: Vec<u8>,
    prefix_read: usize,
}

impl PrefixedStream {
    fn new(inner: TcpStream, prefix: Vec<u8>) -> Self {
        Self {
            inner,
            prefix,
            prefix_read: 0,
        }
    }
}

impl tokio::io::AsyncRead for PrefixedStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.prefix_read < self.prefix.len() {
            let remaining = &self.prefix[self.prefix_read..];
            let to_copy = std::cmp::min(remaining.len(), buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.prefix_read += to_copy;
            std::task::Poll::Ready(Ok(()))
        } else {
            std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }
}

impl tokio::io::AsyncWrite for PrefixedStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn relay(
    mut gateway_side: tokio_rustls::server::TlsStream<PrefixedStream>,
    mut target: TcpStream,
    codec: FrameCodec,
    rate_limiter: Arc<ServerRateLimiter>,
) -> Result<(), TransportError> {
    let mut target_buf = vec![0u8; 8192];
    let idle_timeout = Duration::from_secs(IDLE_TIMEOUT_SECS);

    loop {
        tokio::select! {
            frame = tokio::time::timeout(idle_timeout, codec.read_frame(&mut gateway_side)) => {
                match frame {
                    Ok(Ok(data)) => {
                        while !rate_limiter.try_consume(data.len() as u64).await {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        target.write_all(&data).await?;
                    }
                    Ok(Err(TransportError::Io(e)))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        debug!("client disconnected");
                        return Ok(());
                    }
                    Ok(Err(TransportError::ReplayDetected(c))) => {
                        warn!("replay detected: counter {}", c);
                        metrics::inc_replay_detected();
                        return Ok(());
                    }
                    Ok(Err(TransportError::RekeyNeeded(c, threshold))) => {
                        warn!("rekey needed: counter {} >= {} — initiating rekey handshake", c, threshold);
                        let new_key = FrameCodec::generate_key();
                        let new_kid = codec.kid().wrapping_add(1);
                        match send_rekey_init(&codec, &mut gateway_side, new_kid, &new_key).await {
                            Ok(()) => {
                                match server_handle_rekey_ack(&codec, &mut gateway_side, new_kid, &new_key).await {
                                    Ok(()) => {
                                        info!("rekey completed: new kid={}", new_kid);
                                        metrics::inc_rekey();
                                        continue;
                                    }
                                    Err(e) => {
                                        warn!("rekey ack failed: {:?}", e);
                                        return Ok(());
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("rekey init send failed: {:?}", e);
                                return Ok(());
                            }
                        }
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        debug!("idle timeout ({}s)", IDLE_TIMEOUT_SECS);
                        return Ok(());
                    }
                }
            }

            result = tokio::time::timeout(idle_timeout, target.read(&mut target_buf)) => {
                match result {
                    Ok(Ok(n)) => {
                        if n == 0 {
                            debug!("target closed connection");
                            return Ok(());
                        }
                        while !rate_limiter.try_consume(n as u64).await {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        codec.write_frame(&mut gateway_side, &target_buf[..n]).await?;
                    }
                    Ok(Err(e)) => return Err(TransportError::Io(e)),
                    Err(_) => {
                        debug!("idle timeout ({}s)", IDLE_TIMEOUT_SECS);
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn load_tls_config(cert_path: &str, key_path: &str) -> anyhow::Result<ServerConfig> {
    let cert_data = fs::read(cert_path)?;
    let key_data = fs::read(key_path)?;

    let certs: Vec<_> = certs(&mut &cert_data[..]).collect::<Result<_, _>>()?;

    let key = private_key(&mut &key_data[..])?.ok_or_else(|| {
        anyhow::anyhow!("no private key found in {}", key_path)
    })?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(config)
}

fn parse_hex_key(hex: &str) -> anyhow::Result<[u8; 32]> {
    if hex.len() != 64 {
        anyhow::bail!("--secret must be 64 hex chars (32 bytes), got {}", hex.len());
    }
    let mut key = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let byte_str = std::str::from_utf8(chunk)?;
        key[i] = u8::from_str_radix(byte_str, 16)?;
    }
    Ok(key)
}
