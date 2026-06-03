/// Сервер-шлюз (gateway).
///
/// Принимает TLS-соединения от клиентских transport-core модулей.
/// Для каждого соединения:
///   1. Читает первый фрейм — CONNECT host:port
///   2. Устанавливает TCP-соединение с целевым сервером
///   3. Отправляет OK клиенту
///   4. Проксирует данные в обоих направлениях (relay)
///
/// Запуск:
///   gateway --cert cert.pem --key key.pem --bind 0.0.0.0:443 --secret <32-байт hex>

use std::net::SocketAddr;
use std::sync::Arc;
use std::fs;

use clap::Parser;
use rustls::ServerConfig;
use rustls_pemfile::{certs, private_key};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, warn};

use transport_core::protocol::FrameCodec;
use transport_core::error::TransportError;

#[derive(Parser)]
#[command(name = "gateway", about = "Isolated Browser Gateway Server")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:443")]
    bind: SocketAddr,

    #[arg(long, help = "Path to TLS certificate PEM")]
    cert: String,

    #[arg(long, help = "Path to TLS private key PEM")]
    key: String,

    /// 32-байтовый ключ шифрования в hex (64 символа)
    #[arg(long, help = "Hex-encoded 32-byte shared secret")]
    secret: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "gateway=info".into()),
        )
        .init();

    let args = Args::parse();

    let key = parse_hex_key(&args.secret)?;

    let tls_config = load_tls_config(&args.cert, &args.key)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = TcpListener::bind(args.bind).await?;
    info!("gateway listening on {}", args.bind);

    loop {
        let (tcp, peer) = listener.accept().await?;
        debug!("new connection from {}", peer);

        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            match acceptor.accept(tcp).await {
                Ok(tls_stream) => {
                    if let Err(e) = handle_client(tls_stream, key).await {
                        warn!("client {} error: {}", peer, e);
                    }
                }
                Err(e) => warn!("TLS accept error from {}: {}", peer, e),
            }
        });
    }
}

async fn handle_client(
    mut stream: tokio_rustls::server::TlsStream<TcpStream>,
    key: [u8; 32],
) -> Result<(), TransportError> {
    let codec = FrameCodec::new(&key);

    // Первый фрейм: "CONNECT host:port"
    let frame = codec.read_frame(&mut stream).await?;
    let cmd = String::from_utf8(frame)
        .map_err(|_| TransportError::Protocol("invalid CONNECT frame".into()))?;

    if !cmd.starts_with("CONNECT ") {
        codec.write_frame(&mut stream, b"ERR bad command").await?;
        return Err(TransportError::Protocol(format!("unexpected command: {}", cmd)));
    }

    let target = cmd.trim_start_matches("CONNECT ").trim();
    debug!("→ {}", target);

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

    relay(stream, target_stream, codec).await
}

async fn relay(
    mut gateway_side: tokio_rustls::server::TlsStream<TcpStream>,
    mut target: TcpStream,
    codec: FrameCodec,
) -> Result<(), TransportError> {
    let mut target_buf = vec![0u8; 8192];

    loop {
        tokio::select! {
            // gateway → target (расшифровываем фрейм, пишем в target)
            frame = codec.read_frame(&mut gateway_side) => {
                match frame {
                    Ok(data) => target.write_all(&data).await?,
                    Err(TransportError::Io(e))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        debug!("client disconnected");
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                }
            }

            // target → gateway (читаем из target, шифруем фрейм)
            n = target.read(&mut target_buf) => {
                let n = n?;
                if n == 0 {
                    debug!("target closed connection");
                    return Ok(());
                }
                codec.write_frame(&mut gateway_side, &target_buf[..n]).await?;
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
