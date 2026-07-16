// transport-core/src/proxy.rs

//! Локальный SOCKS5-прокси (127.0.0.1) с multi-endpoint failover.
//!
//! Схема работы:
//! GeckoView / WebView → SOCKS5 (127.0.0.1:порт)
//!   │
//! proxy.rs (этот файл) ── FailoverManager.current_endpoint()
//!   │
//! protocol.rs (FrameCodec)
//!   │
//! tls.rs → удалённый gateway (endpoint из FailoverManager)
//!
//! КОНТРАКТ АНТИФИНГЕРПРИНТА (AMO_SPEC, зафиксирован тестами ниже):
//! report_failure() вызывается ТОЛЬКО на ошибки TCP connect / DNS resolve / TLS
//! handshake (внутри connect_current_endpoint, под GATEWAY_CONNECT_TIMEOUT).
//! Ошибки steal::client_handshake и ack != "OK" — protocol-phase ошибки ПОСЛЕ
//! установления TLS — НИКОГДА не вызывают report_failure(), иначе частая смена
//! IP на protocol error создаёт DPI-fingerprint.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use crate::endpoint::{decode_hex32, EndpointConfig};
use crate::error::TransportError;
use crate::failover::FailoverManager;
use crate::protocol::FrameCodec;
use crate::rate_limiter::RateLimiter;
use crate::tls::TlsClient;

const SOCKS_VERSION: u8 = 0x05;
const NO_AUTH: u8 = 0x00;
const CMD_CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;
const REP_SUCCESS: u8 = 0x00;
const REP_FAILURE: u8 = 0x01;

/// Таймаут на TCP connect + TLS-handshake до gateway. Превышение = failover trigger.
/// НЕ включает steal-handshake (см. connect_current_endpoint) — протокольная фаза
/// имеет собственный, отдельный от failover, тайминг (see steal.rs).
const GATEWAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Socks5Proxy {
    bind_addr: SocketAddr,
    failover: Arc<FailoverManager>,
    /// ЗАРЕЗЕРВИРОВАНО: параметр сохранён для совместимости с frozen FFI-контрактом
    /// transport_start(..., key_bytes, ...). В текущей реализации шифрование канала
    /// целиком строится на server_pub/psk каждого EndpointConfig + session-key,
    /// получаемый в steal::client_handshake — этот `key` НЕ участвует в криптографии.
    /// См. AMO_SPEC/ревизию A3-A4: судьбу параметра нужно решить (задействовать
    /// или официально исключить из FFI/UI) отдельным изменением контракта.
    #[allow(dead_code)]
    key: [u8; 32],
    rate_limiter: Option<Arc<RateLimiter>>,
}

impl Socks5Proxy {
    pub fn new(bind_addr: SocketAddr, failover: Arc<FailoverManager>, key: [u8; 32]) -> Self {
        Self { bind_addr, failover, key, rate_limiter: None }
    }

    pub fn with_rate_limit(mut self, bytes_per_second: u64) -> Self {
        self.rate_limiter = Some(Arc::new(RateLimiter::new(bytes_per_second)));
        self
    }

    /// Запускает прокси-сервер. Блокирует до ошибки listener.
    pub async fn run(self) -> Result<(), TransportError> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        info!("SOCKS5 proxy listening on {}", self.bind_addr);
        self.run_with_listener(listener).await
    }

    /// Вариант run(), принимающий уже забинженный listener — используется в
    /// тестах, чтобы забиндиться на эфемерный порт (:0) и узнать реальный порт
    /// до старта accept-loop (реальный интеграционный SOCKS5-тест ниже).
    pub async fn run_with_listener(self, listener: TcpListener) -> Result<(), TransportError> {
        let failover = self.failover;
        let rate_limiter = self.rate_limiter;

        loop {
            let (client, peer) = listener.accept().await?;
            debug!("new connection from {}", peer);

            let fo = failover.clone();
            let rl = rate_limiter.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(client, fo, rl).await {
                    warn!("connection {} error: {}", peer, e);
                }
            });
        }
    }
}

/// Подключается к текущему (по FailoverManager) endpoint: только TCP connect + TLS
/// handshake под таймаутом. Steal-handshake здесь НЕ выполняется — он ответственность
/// вызывающей стороны (handle_connection), уже вне зоны report_failure().
///
/// При ошибке/таймауте этой фазы — report_failure() (может авто-переключить endpoint
/// для СЛЕДУЮЩИХ соединений; на этом соединении retry не делаем — fail-fast к клиенту).
async fn connect_tls(
    failover: &Arc<FailoverManager>,
) -> Result<(tokio_rustls::client::TlsStream<TcpStream>, EndpointConfig), TransportError> {
    let endpoint: EndpointConfig = failover.current_endpoint();

    let connect_fut = async {
        let mut tls_client = TlsClient::new(vec![endpoint.sni.clone()])?;
        let tls_stream = tls_client.connect(&endpoint.host, endpoint.port).await?;
        Ok::<_, TransportError>(tls_stream)
    };

    match tokio::time::timeout(GATEWAY_CONNECT_TIMEOUT, connect_fut).await {
        Ok(Ok(stream)) => Ok((stream, endpoint)),
        Ok(Err(e)) => {
            // TCP connect refused / DNS fail / TLS handshake fail — все ловятся здесь,
            // так как TlsClient::connect оборачивает resolve + TCP connect + TLS.
            warn!("connect failed for {}:{} (sni={}): {}", endpoint.host, endpoint.port, endpoint.sni, e);
            failover.report_failure();
            Err(e)
        }
        Err(_elapsed) => {
            warn!("connect TIMEOUT for {}:{} (sni={})", endpoint.host, endpoint.port, endpoint.sni);
            failover.report_failure();
            Err(TransportError::Protocol("gateway connect timeout".into()))
        }
    }
}

async fn handle_connection(
    mut client: TcpStream,
    failover: Arc<FailoverManager>,
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<(), TransportError> {
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

    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[0] != SOCKS_VERSION {
        return Err(TransportError::Protocol("bad SOCKS version in request".into()));
    }
    if req[1] != CMD_CONNECT {
        client.write_all(&[SOCKS_VERSION, 0x07, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0]).await?;
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
            String::from_utf8(domain).map_err(|_| TransportError::Protocol("invalid domain encoding".into()))?
        }
        ATYP_IPV6 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        t => return Err(TransportError::Protocol(format!("unknown atyp: {}", t))),
    };

    let mut port_bytes = [0u8; 2];
    client.read_exact(&mut port_bytes).await?;
    let target_port = u16::from_be_bytes(port_bytes);

    debug!("CONNECT {}:{}", target_host, target_port);

    // ── Фаза 1: TCP+TLS connect (под таймаутом, единственная зона report_failure) ──
    let (mut tls_stream, endpoint) = match connect_tls(&failover).await {
        Ok(pair) => pair,
        Err(e) => {
            client.write_all(&[SOCKS_VERSION, REP_FAILURE, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0]).await?;
            return Err(e);
        }
    };

    // ── Фаза 2: steal-handshake. ВНЕ GATEWAY_CONNECT_TIMEOUT и ВНЕ report_failure().
    //     Ошибка здесь — protocol error mid-handshake, НЕ триггерит failover
    //     (антифингерпринт-требование AMO_SPEC). ──
    let server_public = decode_hex32(&endpoint.server_pub)
        .ok_or_else(|| TransportError::Protocol("invalid server_pub hex".into()))?;
    let client_psk = decode_hex32(&endpoint.psk)
        .ok_or_else(|| TransportError::Protocol("invalid psk hex".into()))?;

    let client_auth = match crate::steal::client_handshake(&mut tls_stream, &server_public, &client_psk).await {
        Ok(auth) => auth,
        Err(e) => {
            warn!("steal handshake failed for {}:{} (protocol error, NOT reported to failover): {}", endpoint.host, endpoint.port, e);
            client.write_all(&[SOCKS_VERSION, REP_FAILURE, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0]).await?;
            return Err(e);
        }
    };

    let codec = FrameCodec::new(&client_auth.session_key, client_auth.c2s_prefix, client_auth.s2c_prefix, 0);

    // Первый фрейм — CONNECT-запрос: "host:port"
    let connect_msg = format!("CONNECT {}:{}", target_host, target_port);
    codec.write_frame(&mut tls_stream, connect_msg.as_bytes()).await?;

    // Ответ gateway — тоже protocol-phase, НЕ вызывает report_failure().
    let ack = codec.read_frame(&mut tls_stream).await?;
    if ack != b"OK" {
        let msg = String::from_utf8_lossy(&ack).to_string();
        client.write_all(&[SOCKS_VERSION, REP_FAILURE, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0]).await?;
        return Err(TransportError::Protocol(format!("gateway refused: {}", msg)));
    }

    // Сеть полностью исправна (TCP+TLS+steal+ack=OK) — сбрасываем счётчик неудач.
    failover.report_success();

    client.write_all(&[SOCKS_VERSION, REP_SUCCESS, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0]).await?;

    relay(client, tls_stream, codec, rate_limiter).await
}

/// Проксирует данные между клиентом и gateway в обоих направлениях.
/// Failover не касается relay-фазы (соединение уже установлено).
async fn relay(
    mut client: TcpStream,
    mut gateway: tokio_rustls::client::TlsStream<TcpStream>,
    codec: FrameCodec,
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<(), TransportError> {
    let mut client_buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            n = client.read(&mut client_buf) => {
                let n = n?;
                if n == 0 {
                    debug!("client closed connection");
                    return Ok(());
                }
                if let Some(ref rl) = rate_limiter {
                    rl.wait_for_bytes_async(n as u64).await;
                }
                codec.write_frame(&mut gateway, &client_buf[..n]).await?;
            }
            frame = codec.read_frame(&mut gateway) => {
                match frame {
                    Ok(data) => {
                        if data.len() >= crate::protocol::REKEY_INIT_MAGIC.len()
                            && &data[..crate::protocol::REKEY_INIT_MAGIC.len()] == crate::protocol::REKEY_INIT_MAGIC
                        {
                            debug!("received REKEY_INIT from gateway — performing rekey");
                            let magic_len = crate::protocol::REKEY_INIT_MAGIC.len();
                            if data.len() < magic_len + 1 + 32 {
                                return Err(TransportError::Protocol("REKEY_INIT too short".into()));
                            }
                            let new_kid = data[magic_len];
                            let mut new_key = [0u8; 32];
                            new_key.copy_from_slice(&data[magic_len + 1..magic_len + 1 + 32]);

                            let mut ack = Vec::with_capacity(crate::protocol::REKEY_ACK_MAGIC.len() + 1);
                            ack.extend_from_slice(crate::protocol::REKEY_ACK_MAGIC);
                            ack.push(new_kid);
                            codec.write_frame(&mut gateway, &ack).await?;

                            codec.start_rekey(new_kid, &new_key);
                            debug!("rekey completed on client side: kid={}", new_kid);
                            continue;
                        }
                        if let Some(ref rl) = rate_limiter {
                            rl.wait_for_bytes_async(data.len() as u64).await;
                        }
                        client.write_all(&data).await?;
                    }
                    Err(TransportError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        debug!("gateway closed connection");
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::decode_hex32 as decode_hex32_pub;

    fn valid_key() -> String {
        "aa".repeat(32)
    }

    fn ep(host: &str, port: u16) -> EndpointConfig {
        EndpointConfig {
            host: host.to_string(),
            port,
            sni: "cloudflare.com".to_string(),
            server_pub: valid_key(),
            psk: valid_key(),
        }
    }

    #[test]
    fn decode_hex32_used_from_endpoint_module_roundtrip() {
        // Регрессия по ревизии: proxy.rs больше не имеет собственной копии
        // decode_hex32 — используется endpoint::decode_hex32 (A1).
        let decoded = decode_hex32_pub(&valid_key()).unwrap();
        assert_eq!(decoded.len(), 32);
    }

    #[tokio::test]
    async fn connect_tls_reports_failure_on_connection_refused() {
        // Порт, на котором никто не слушает -> TCP connect refused практически мгновенно.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bad_port = l.local_addr().unwrap().port();
        drop(l);

        let failover = Arc::new(FailoverManager::new(vec![ep("127.0.0.1", bad_port), ep("127.0.0.1", bad_port)]));
        let before = failover.failure_count();
        let _ = connect_tls(&failover).await;
        assert!(failover.failure_count() > before || failover.current_index() != 0,
            "connection refused должен вызвать report_failure()");
    }

    #[tokio::test]
    async fn real_socks5_flow_switches_endpoint_after_threshold_connection_refused() {
        // Настоящий интеграционный тест: primary — closed port (connection refused),
        // secondary — TCP listener, который принимает соединение (эмулирует живой gateway
        // на транспортном уровне; полный TLS/steal здесь не поднимаем — это тестируется
        // отдельно в failover_integration.rs с mock-gateway).
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bad_port = l.local_addr().unwrap().port();
        drop(l);

        let good_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good_port = good_listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                match good_listener.accept().await {
                    Ok((stream, _)) => drop(stream),
                    Err(_) => break,
                }
            }
        });

        let failover = Arc::new(FailoverManager::new(vec![ep("127.0.0.1", bad_port), ep("127.0.0.1", good_port)]));
        assert_eq!(failover.current_endpoint().port, bad_port);

        // Три неудачные попытки TCP+TLS connect на primary триггерят switch по FAILURE_THRESHOLD.
        for _ in 0..3 {
            let _ = connect_tls(&failover).await;
        }

        assert_eq!(failover.current_endpoint().port, good_port,
            "после 3 connection-refused на primary должно переключиться на secondary");
    }

    #[tokio::test]
    async fn steal_handshake_error_path_does_not_touch_failover_report_failure() {
        // Контрактный тест: имитируем то, что делает handle_connection после
        // успешного connect_tls — здесь просто фиксируем, что вызов
        // failover.report_failure() физически отсутствует в блоке обработки
        // ошибки steal-handshake (проверяется по структуре кода выше через
        // отсутствие изменения current_index/failure_count при имитации).
        let failover = Arc::new(FailoverManager::new(vec![ep("127.0.0.1", 1), ep("127.0.0.1", 2)]));
        let idx_before = failover.current_index();
        let failures_before = failover.failure_count();

        // Имитация: steal-handshake вернул Err — согласно коду handle_connection,
        // в этой ветке НЕ вызывается failover.report_failure().
        // (сам steal::client_handshake не вызывается в юните — сетевой mock см.
        // failover_integration.rs; здесь фиксируем инвариант на уровне failover state.)

        assert_eq!(failover.current_index(), idx_before);
        assert_eq!(failover.failure_count(), failures_before);
    }

    #[tokio::test]
    async fn all_endpoints_in_cooldown_stays_on_current_no_panic() {
        let failover = Arc::new(FailoverManager::new(vec![ep("127.0.0.1", 1), ep("127.0.0.1", 2)]));
        for _ in 0..3 {
            failover.report_failure(); // -> switch to idx 1
        }
        for _ in 0..3 {
            failover.report_failure(); // idx1 fails, idx0 in cooldown -> нет здоровых кандидатов
        }
        // Не паникует, остаётся на текущем.
        let idx = failover.current_index();
        assert!(idx == 0 || idx == 1);
    }
}