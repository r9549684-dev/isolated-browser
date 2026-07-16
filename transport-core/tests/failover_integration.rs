// transport-core/tests/failover_integration.rs

//! Реальный интеграционный тест через SOCKS5-протокол (не юнит-обёртка вокруг
//! FailoverManager). Поднимает Socks5Proxy на эфемерном порту, подключается
//! настоящим TCP-клиентом по SOCKS5-протоколу и проверяет сквозное поведение.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use transport_core::endpoint::EndpointConfig;
use transport_core::failover::FailoverManager;
use transport_core::proxy::Socks5Proxy;

fn dummy_endpoint(host: &str, port: u16) -> EndpointConfig {
    EndpointConfig {
        host: host.to_string(),
        port,
        sni: "cloudflare.com".to_string(),
        server_pub: "aa".repeat(32),
        psk: "bb".repeat(32),
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// Отправляет SOCKS5 handshake (NO_AUTH) + CONNECT-запрос на target, читает reply.
/// Возвращает REP-код (0x00 success, иначе failure).
async fn socks5_connect(proxy_port: u16, target_host: &str, target_port: u16) -> std::io::Result<u8> {
    let addr: SocketAddr = format!("127.0.0.1:{}", proxy_port).parse().unwrap();
    let mut stream = TcpStream::connect(addr).await?;

    // Greeting: version 5, 1 method, NO_AUTH
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await?;
    assert_eq!(resp, [0x05, 0x00], "server должен принять NO_AUTH");

    // CONNECT request, ATYP_DOMAIN
    let mut req = vec![0x05, 0x01, 0x00, 0x03];
    req.push(target_host.len() as u8);
    req.extend_from_slice(target_host.as_bytes());
    req.extend_from_slice(&target_port.to_be_bytes());
    stream.write_all(&req).await?;

    let mut reply_header = [0u8; 4];
    stream.read_exact(&mut reply_header).await?;
    let mut addr_buf = [0u8; 6]; // ATYP_IPV4 + 4 bytes IP + 2 bytes port (упрощённый reply формата proxy.rs)
    stream.read_exact(&mut addr_buf).await?;

    Ok(reply_header[1])
}

/// Гоняет N попыток connect через прокси, чтобы триггернуть failover threshold,
/// затем проверяет, что FailoverManager реально переключился.
#[tokio::test]
async fn socks5_client_triggers_failover_after_repeated_connection_refused() {
    let bad_port = free_port(); // никто не слушает -> connection refused
    let good_port = free_port();

    // secondary — реальный TCP listener, принимающий соединения (эмулирует живой gateway
    // на транспортном уровне; полный TLS handshake здесь не проходит, поэтому итоговый
    // SOCKS5-ответ клиенту всё равно будет failure на TLS-фазе, но connect_tls успеет
    // отработать TCP connect до того, как TLS упадёт — секция REP кода не проверяется
    // в этом тесте намеренно, тестируем только переключение индекса).
    let good_listener = TcpListener::bind(format!("127.0.0.1:{good_port}")).await.unwrap();
    tokio::spawn(async move {
        loop {
            match good_listener.accept().await {
                Ok((stream, _)) => drop(stream),
                Err(_) => break,
            }
        }
    });

    let endpoints = vec![dummy_endpoint("127.0.0.1", bad_port), dummy_endpoint("127.0.0.1", good_port)];
    let failover = Arc::new(FailoverManager::new(endpoints));
    let key = [0u8; 32];

    let proxy_port = free_port();
    let listener = TcpListener::bind(format!("127.0.0.1:{proxy_port}")).await.unwrap();
    let proxy = Socks5Proxy::new(format!("127.0.0.1:{proxy_port}").parse().unwrap(), failover.clone(), key);

    tokio::spawn(proxy.run_with_listener(listener));
    tokio::time::sleep(Duration::from_millis(50)).await; // дать accept-loop запуститься

    assert_eq!(failover.current_endpoint().port, bad_port);

    // Три реальных SOCKS5-клиентских подключения через прокси на bad primary.
    for _ in 0..3 {
        let result = timeout(Duration::from_secs(15), socks5_connect(proxy_port, "example.com", 80)).await;
        assert!(result.is_ok(), "SOCKS5-клиент должен получить ответ (success или failure), не таймаут соединения к прокси");
    }

    assert_eq!(
        failover.current_endpoint().port, good_port,
        "после 3 connection-refused через реальный SOCKS5-путь должно произойти переключение"
    );
}

#[tokio::test]
async fn socks5_rejects_non_connect_command() {
    let endpoints = vec![dummy_endpoint("127.0.0.1", free_port())];
    let failover = Arc::new(FailoverManager::new(endpoints));
    let key = [0u8; 32];

    let proxy_port = free_port();
    let listener = TcpListener::bind(format!("127.0.0.1:{proxy_port}")).await.unwrap();
    let proxy = Socks5Proxy::new(format!("127.0.0.1:{proxy_port}").parse().unwrap(), failover, key);
    tokio::spawn(proxy.run_with_listener(listener));
    tokio::time::sleep(Duration::from_millis(50)).await;

    let addr: SocketAddr = format!("127.0.0.1:{proxy_port}").parse().unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await.unwrap();
    assert_eq!(resp, [0x05, 0x00]);

    // BIND command (0x02) вместо CONNECT (0x01) — должен быть отклонён.
    stream.write_all(&[0x05, 0x02, 0x00, 0x01, 127, 0, 0, 1, 0, 80]).await.unwrap();
    let mut reply = [0u8; 10];
    let n = stream.read(&mut reply).await.unwrap();
    assert!(n > 0);
    assert_eq!(reply[1], 0x07, "должен вернуться REP код 'command not supported'");
}

#[tokio::test]
async fn socks5_rejects_connection_without_no_auth_method() {
    let endpoints = vec![dummy_endpoint("127.0.0.1", free_port())];
    let failover = Arc::new(FailoverManager::new(endpoints));
    let key = [0u8; 32];

    let proxy_port = free_port();
    let listener = TcpListener::bind(format!("127.0.0.1:{proxy_port}")).await.unwrap();
    let proxy = Socks5Proxy::new(format!("127.0.0.1:{proxy_port}").parse().unwrap(), failover, key);
    tokio::spawn(proxy.run_with_listener(listener));
    tokio::time::sleep(Duration::from_millis(50)).await;

    let addr: SocketAddr = format!("127.0.0.1:{proxy_port}").parse().unwrap();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    // Заявляем только username/password auth (0x02), без NO_AUTH (0x00).
    stream.write_all(&[0x05, 0x01, 0x02]).await.unwrap();
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await.unwrap();
    assert_eq!(resp, [0x05, 0xFF], "должен вернуть 0xFF (no acceptable methods)");
}