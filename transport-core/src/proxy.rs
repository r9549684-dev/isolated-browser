/// Локальный SOCKS5-прокси (127.0.0.1).
///
/// Схема работы:
///   GeckoView / WebView  →  SOCKS5 (127.0.0.1:порт)
///                              │
///                        proxy.rs (этот файл)
///                              │
///                        protocol.rs (FrameCodec)
///                              │
///                        tls.rs → удалённый gateway

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use crate::error::TransportError;
use crate::protocol::FrameCodec;
use crate::rate_limiter::RateLimiter;
use crate::tls::TlsClient;

const SOCKS_VERSION: u8 = 0x05;
const NO_AUTH: u8       = 0x00;
const CMD_CONNECT: u8   = 0x01;
const ATYP_IPV4: u8     = 0x01;
const ATYP_DOMAIN: u8   = 0x03;
const ATYP_IPV6: u8     = 0x04;
const REP_SUCCESS: u8   = 0x00;
const REP_FAILURE: u8   = 0x01;

pub struct Socks5Proxy {
    bind_addr: SocketAddr,
    gateway_host: String,
    gateway_port: u16,
    key: [u8; 32],
    sni_pool: Vec<String>,
    server_public: [u8; 32],
    rate_limiter: Option<Arc<RateLimiter>>,
}

impl Socks5Proxy {
    pub fn new(
        bind_addr: SocketAddr,
        gateway_host: impl Into<String>,
        gateway_port: u16,
        key: [u8; 32],
        sni_pool: Vec<String>,
        server_public: [u8; 32],
    ) -> Self {
        Self {
            bind_addr,
            gateway_host: gateway_host.into(),
            gateway_port,
            key,
            sni_pool,
            server_public,
            rate_limiter: None,
        }
    }

    pub fn with_rate_limit(mut self, bytes_per_second: u64) -> Self {
        self.rate_limiter = Some(Arc::new(RateLimiter::new(bytes_per_second)));
        self
    }

    /// Запускает прокси-сервер. Блокирует до ошибки listener.
    pub async fn run(self) -> Result<(), TransportError> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        info!("SOCKS5 proxy listening on {}", self.bind_addr);

        let gateway_host = std::sync::Arc::new(self.gateway_host);
        let key = self.key;
        let gateway_port = self.gateway_port;
        let sni_pool = self.sni_pool;
        let server_public = self.server_public;
        let rate_limiter = self.rate_limiter;

        loop {
            let (client, peer) = listener.accept().await?;
            debug!("new connection from {}", peer);

            let gw_host = gateway_host.clone();
            let sni = sni_pool.clone();
            let rl = rate_limiter.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    handle_connection(client, &gw_host, gateway_port, key, sni, server_public, rl).await
                {
                    warn!("connection {} error: {}", peer, e);
                }
            });
        }
    }
}

async fn handle_connection(
    mut client: TcpStream,
    gateway_host: &str,
    gateway_port: u16,
    key: [u8; 32],
    sni_pool: Vec<String>,
    server_public: [u8; 32],
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<(), TransportError> {
    // ── 1. SOCKS5 handshake ───────────────────────────────────────────────
    let mut header = [0u8; 2];
    client.read_exact(&mut header).await?;

    if header[0] != SOCKS_VERSION {
        return Err(TransportError::Protocol("not SOCKS5".into()));
    }

    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    client.read_exact(&mut methods).await?;

    if !methods.contains(&NO_AUTH) {
        client.write_all(&[SOCKS_VERSION, 0xFF]).await?;
        return Err(TransportError::Protocol("no acceptable auth method".into()));
    }
    client.write_all(&[SOCKS_VERSION, NO_AUTH]).await?;

    // ── 2. SOCKS5 request ────────────────────────────────────────────────
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;

    if req[0] != SOCKS_VERSION {
        return Err(TransportError::Protocol("bad SOCKS version in request".into()));
    }
    if req[1] != CMD_CONNECT {
        client
            .write_all(&[SOCKS_VERSION, 0x07, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
            .await?;
        return Err(TransportError::Protocol("only CONNECT supported".into()));
    }

    let target_host = match req[3] {
        ATYP_IPV4 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut domain = vec![0u8; len[0] as usize];
            client.read_exact(&mut domain).await?;
            String::from_utf8(domain)
                .map_err(|_| TransportError::Protocol("invalid domain encoding".into()))?
        }
        ATYP_IPV6 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        t => {
            return Err(TransportError::Protocol(format!("unknown atyp: {}", t)));
        }
    };

    let mut port_bytes = [0u8; 2];
    client.read_exact(&mut port_bytes).await?;
    let target_port = u16::from_be_bytes(port_bytes);

    debug!("CONNECT {}:{}", target_host, target_port);

    // ── 3. Подключаемся к gateway поверх TLS ────────────────────────────
    let mut tls_client = TlsClient::new(sni_pool)?;
    let mut tls_stream = tls_client.connect(gateway_host, gateway_port).await?;

    // ── 4. Steal-oncall auth frame ──────────────────────────────────────
    crate::steal::send_auth_frame(&mut tls_stream, &server_public).await?;

    let codec = FrameCodec::new(&key);

    // Первый фрейм — CONNECT-запрос: "host:port"
    let connect_msg = format!("CONNECT {}:{}", target_host, target_port);
    codec.write_frame(&mut tls_stream, connect_msg.as_bytes()).await?;

    // Ответ gateway
    let ack = codec.read_frame(&mut tls_stream).await?;
    if ack != b"OK" {
        let msg = String::from_utf8_lossy(&ack).to_string();
        client
            .write_all(&[SOCKS_VERSION, REP_FAILURE, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
            .await?;
        return Err(TransportError::Protocol(format!("gateway refused: {}", msg)));
    }

    // Сообщаем клиенту: соединение установлено
    client
        .write_all(&[SOCKS_VERSION, REP_SUCCESS, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
        .await?;

    // ── 4. Прозрачный двунаправленный туннель ────────────────────────────
    relay(client, tls_stream, codec, rate_limiter).await
}

/// Проксирует данные между клиентом и gateway в обоих направлениях.
/// Клиент → codec.write_frame → gateway
/// Gateway → codec.read_frame → клиент
async fn relay(
    mut client: TcpStream,
    mut gateway: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    codec: FrameCodec,
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<(), TransportError> {
    let mut client_buf = vec![0u8; 8192];

    loop {
        tokio::select! {
            // Клиент → gateway
            n = client.read(&mut client_buf) => {
                let n = n?;
                if n == 0 {
                    debug!("client closed connection");
                    return Ok(());
                }
                
                // Rate limiting (client-side) — async wait
                if let Some(ref rl) = rate_limiter {
                    rl.wait_for_bytes_async(n as u64).await;
                }
                
                codec.write_frame(&mut gateway, &client_buf[..n]).await?;
            }

            // Gateway → клиент
            frame = codec.read_frame(&mut gateway) => {
                match frame {
                    Ok(data) => {
                        // Rate limiting (client-side) — async wait
                        if let Some(ref rl) = rate_limiter {
                            rl.wait_for_bytes_async(data.len() as u64).await;
                        }
                        
                        client.write_all(&data).await?;
                    }
                    Err(TransportError::Io(e))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        debug!("gateway closed connection");
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
}
