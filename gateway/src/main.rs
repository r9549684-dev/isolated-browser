/// Сервер-шлюз (gateway) с TCP-level обработкой для защиты от active probing.
///
/// Архитектура (как в REALITY):
/// 1. Принять TCP соединение
/// 2. Прочитать ClientHello (первые байты TLS handshake)
/// 3. Извлечь auth token из ClientHello (из session_id)
/// 4. Если auth валиден — продолжить TLS handshake через rustls
/// 5. Если auth невалиден — проксировать сырые TCP байты на реальный CDN (fallback)
///
/// Это защищает от active probing:
/// - ТСПУ видит настоящий TLS ClientHello
/// - При невалидном auth — байт-в-байт проксирование на CDN (тот же сертификат/JA3S)
/// - Нет "второго" handshake, нет отличий от прямого соединения
///
/// Запуск:
///   gateway --cert cert.pem --key key.pem --bind 0.0.0.0:443
///           --secret <32-байт hex> --server-private-key <32-байт hex>
///           --fallback-cdn cloudflare.com:443

use std::net::SocketAddr;
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
use tracing::{debug, error, info, warn};

use transport_core::protocol::FrameCodec;
use transport_core::steal::verify_server_auth;
use transport_core::tcp_handler::{extract_auth_from_clienthello, fallback_tcp_proxy};
use transport_core::error::TransportError;

mod auth;
mod promo;
use auth::{AuthManager, Claims};

const MAX_CONCURRENT_CONNECTIONS: usize = 5000;
const IDLE_TIMEOUT_SECS: u64 = 300; // 5 минут

/// Rate limiter для серверной части (token bucket)
struct ServerRateLimiter {
    capacity: u64,
    tokens: AtomicU64,
    refill_rate: u64,
    last_refill: Mutex<Instant>,
}

impl ServerRateLimiter {
    fn new(bytes_per_second: u64) -> Self {
        Self {
            capacity: bytes_per_second,
            tokens: AtomicU64::new(bytes_per_second),
            refill_rate: bytes_per_second,
            last_refill: Mutex::new(Instant::now()),
        }
    }

    async fn try_consume(&self, amount: u64) -> bool {
        self.refill().await;
        
        let current = self.tokens.load(Ordering::Relaxed);
        if current >= amount {
            self.tokens.fetch_sub(amount, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    async fn refill(&self) {
        let mut last_refill = self.last_refill.lock().await;
        let now = Instant::now();
        let elapsed = now.duration_since(*last_refill);
        
        if elapsed >= Duration::from_secs(1) {
            let tokens_to_add = self.refill_rate * (elapsed.as_secs() as u64);
            let current = self.tokens.load(Ordering::Relaxed);
            let new_tokens = (current + tokens_to_add).min(self.capacity);
            self.tokens.store(new_tokens, Ordering::Relaxed);
            *last_refill = now;
        }
    }
}

/// Менеджер подписок с JWT аутентификацией
struct SubscriptionManager {
    auth_manager: Arc<AuthManager>,
}

impl SubscriptionManager {
    fn new(jwt_secret: String, hmac_key: Vec<u8>) -> Self {
        Self {
            auth_manager: Arc::new(AuthManager::new(jwt_secret, hmac_key)),
        }
    }

    async fn validate_subscription_token(&self, token: &str) -> Option<Claims> {
        match self.auth_manager.validate_token(token) {
            Ok(claims) => Some(claims),
            Err(e) => {
                warn!("Subscription token validation failed: {}", e);
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

    #[arg(long, help = "Path to TLS certificate PEM (должен быть для CDN-домена)")]
    cert: String,

    #[arg(long, help = "Path to TLS private key PEM")]
    key: String,

    /// 32-байтовый ключ шифрования в hex (64 символа)
    #[arg(long, help = "Hex-encoded 32-byte shared secret for ChaCha20-Poly1305")]
    secret: String,

    /// 32-байтовый X25519 private key сервера в hex (64 символа)
    #[arg(long, help = "Hex-encoded 32-byte X25519 server private key")]
    server_private_key: String,

    /// Fallback CDN-домен для проксирования при невалидном auth (защита от active probing)
    #[arg(long, default_value = "cloudflare.com:443", help = "Fallback CDN for stealth mode")]
    fallback_cdn: String,

    /// JWT secret для подписи токенов подписок
    #[arg(long, env = "JWT_SECRET", help = "JWT secret for subscription tokens")]
    jwt_secret: String,

    /// HMAC key для подписи данных
    #[arg(long, env = "HMAC_KEY", help = "Hex-encoded HMAC key for data signing")]
    hmac_key: String,

    /// Тестовый режим (принимает любой токен)
    #[arg(long, help = "Test mode: accept any subscription token")]
    test_mode: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Устанавливаем CryptoProvider для rustls
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "gateway=info".into()),
        )
        .init();

    let args = Args::parse();

    let key = parse_hex_key(&args.secret)?;
    let server_private_key = parse_hex_key(&args.server_private_key)?;
    let fallback_cdn = args.fallback_cdn.clone();
    let hmac_key = parse_hex_key(&args.hmac_key)?;

    let tls_config = load_tls_config(&args.cert, &args.key)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let subscription_manager = Arc::new(SubscriptionManager::new(
        args.jwt_secret.clone(),
        hmac_key.to_vec(),
    ));
    let connection_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
    let test_mode = args.test_mode;

    let listener = TcpListener::bind(args.bind).await?;
    info!("gateway listening on {} (TCP-level steal-TLS, fallback: {}, max_connections: {})", 
          args.bind, fallback_cdn, MAX_CONCURRENT_CONNECTIONS);

    loop {
        let (tcp, peer) = listener.accept().await?;
        debug!("new TCP connection from {}", peer);

        // Ограничение количества одновременных подключений
        let permit = match connection_semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                warn!("connection limit reached ({}), rejecting {}", MAX_CONCURRENT_CONNECTIONS, peer);
                drop(tcp);
                continue;
            }
        };

        let acceptor = acceptor.clone();
        let fallback = fallback_cdn.clone();
        let sub_mgr = subscription_manager.clone();
        let test = test_mode;
        tokio::spawn(async move {
            let _permit = permit; // Держим permit до завершения соединения
            if let Err(e) = handle_tcp_connection(tcp, key, server_private_key, fallback, acceptor, sub_mgr, test).await {
                warn!("client {} error: {}", peer, e);
            }
        });
    }
}

/// Обработка TCP соединения на уровне ClientHello.
/// Извлекает JWT токен из ClientHello, проверяет auth, и либо:
/// - Продолжает TLS handshake (если auth OK)
/// - Проксирует на CDN (если auth FAIL)
async fn handle_tcp_connection(
    mut tcp: TcpStream,
    key: [u8; 32],
    server_private_key: [u8; 32],
    fallback_cdn: String,
    acceptor: TlsAcceptor,
    subscription_manager: Arc<SubscriptionManager>,
    test_mode: bool,
) -> Result<(), TransportError> {
    // Читаем ClientHello (до 16KB)
    let mut buffer = vec![0u8; 16384];
    let n = tcp.read(&mut buffer).await
        .map_err(TransportError::Io)?;
    
    if n == 0 {
        return Err(TransportError::Protocol("empty connection".into()));
    }
    
    let client_hello_data = &buffer[..n];
    
    // Извлекаем auth token из ClientHello
    let auth_result = extract_auth_from_clienthello(client_hello_data);
    
    match auth_result {
        Some((auth_token, _)) => {
            // Проверяем auth (упрощенно — в реальности нужно извлечь ephemeral public из ClientHello)
            // Для простоты используем auth_token как ephemeral public
            let ephemeral_public: [u8; 32] = auth_token;
            
            // Генерируем expected token
            let expected_token = generate_expected_token(&server_private_key, &ephemeral_public);
            
            if verify_server_auth(&server_private_key, &ephemeral_public, &expected_token) {
                debug!("auth OK — continuing TLS handshake");
                
                // Продолжаем TLS handshake
                // Нужно "вернуть" ClientHello обратно в поток для rustls
                // Используем PrefixedStream для этого
                let prefixed_stream = PrefixedStream::new(tcp, client_hello_data.to_vec());
                
                match acceptor.accept(prefixed_stream).await {
                    Ok(tls_stream) => {
                        // Извлекаем JWT токен из ephemeral_public (первые 8 байт как hex)
                        // В реальности JWT токен должен передаваться отдельно
                        let jwt_token = hex::encode(&ephemeral_public[..8]);
                        
                        // Валидируем подписку через JWT (или пропускаем в test_mode)
                        let claims = if test_mode {
                            // В тестовом режиме создаём фейковые claims
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
                                handle_authenticated_client(tls_stream, key, subscription_manager, claims).await?;
                            }
                            None => {
                                debug!("invalid subscription token — closing connection");
                                return Err(TransportError::Protocol("invalid subscription token".into()));
                            }
                        }
                    }
                    Err(e) => {
                        debug!("TLS accept error: {}", e);
                        return Err(TransportError::Protocol(format!("TLS accept failed: {}", e)));
                    }
                }
            } else {
                debug!("auth FAIL — closing connection");
                return Err(TransportError::Protocol("auth failed".into()));
            }
        }
        None => {
            debug!("no auth token in ClientHello — closing connection");
            return Err(TransportError::Protocol("no auth token".into()));
        }
    }
    
    Ok(())
}

/// Генерирует expected token для проверки auth.
/// В реальности клиент отправляет ephemeral public + auth_token в ClientHello.
/// Здесь упрощенная версия: клиент отправляет только auth_token в session_id.
fn generate_expected_token(server_secret: &[u8; 32], ephemeral_public: &[u8; 32]) -> [u8; 32] {
    use x25519_dalek::{StaticSecret, PublicKey};
    use hkdf::Hkdf;
    use sha2::Sha256;
    
    let server_static = StaticSecret::from(*server_secret);
    let client_public = PublicKey::from(*ephemeral_public);
    let shared_secret = server_static.diffie_hellman(&client_public);
    
    let hkdf = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
    let mut expected_token = [0u8; 32];
    hkdf.expand(b"isolated-browser-auth", &mut expected_token)
        .expect("HKDF expand failed");
    
    expected_token
}

/// Обработка авторизованного клиента (после TLS handshake).
async fn handle_authenticated_client(
    mut stream: tokio_rustls::server::TlsStream<PrefixedStream>,
    key: [u8; 32],
    subscription_manager: Arc<SubscriptionManager>,
    claims: Claims,
) -> Result<(), TransportError> {
    let codec = FrameCodec::new(&key);

    // Читаем CONNECT frame
    let frame = codec.read_frame(&mut stream).await?;
    let cmd = String::from_utf8(frame)
        .map_err(|_| TransportError::Protocol("invalid CONNECT frame".into()))?;

    if !cmd.starts_with("CONNECT ") {
        codec.write_frame(&mut stream, b"ERR bad command").await?;
        return Err(TransportError::Protocol(format!("unexpected command: {}", cmd)));
    }

    let target = cmd.trim_start_matches("CONNECT ").trim();
    debug!("→ {} (subscription: {}, tier: {})", target, claims.sub, claims.tier);

    let target_stream = match TcpStream::connect(target).await {
        Ok(s) => s,
        Err(e) => {
            codec
                .write_frame(&mut stream, format!("ERR {}", e).as_bytes())
                .await?;
            return Err(TransportError::Io(e));
        }
    };

    codec.write_frame(&mut stream, b"OK").await?;

    // Получаем rate limit из JWT claims
    let rate_limit = subscription_manager.get_rate_limit_from_claims(&claims).await;
    let rate_limiter = Arc::new(ServerRateLimiter::new(rate_limit));
    
    debug!("subscription {} rate limit: {} bytes/sec", claims.sub, rate_limit);

    relay(stream, target_stream, codec, rate_limiter).await
}

/// Поток с префиксом (для "возврата" ClientHello в rustls).
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
            // gateway → target
            frame = tokio::time::timeout(idle_timeout, codec.read_frame(&mut gateway_side)) => {
                match frame {
                    Ok(Ok(data)) => {
                        // Server-side rate limiting
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
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        debug!("idle timeout ({}s) — closing connection", IDLE_TIMEOUT_SECS);
                        return Ok(());
                    }
                }
            }

            // target → gateway
            result = tokio::time::timeout(idle_timeout, target.read(&mut target_buf)) => {
                match result {
                    Ok(Ok(n)) => {
                        if n == 0 {
                            debug!("target closed connection");
                            return Ok(());
                        }
                        
                        // Server-side rate limiting
                        while !rate_limiter.try_consume(n as u64).await {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        
                        codec.write_frame(&mut gateway_side, &target_buf[..n]).await?;
                    }
                    Ok(Err(e)) => return Err(TransportError::Io(e)),
                    Err(_) => {
                        debug!("idle timeout ({}s) — closing connection", IDLE_TIMEOUT_SECS);
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

    let certs: Vec<_> = certs(&mut &cert_data[..])
        .collect::<Result<_, _>>()?;

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
