use std::sync::Arc;
use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use rustls::pki_types::ServerName;
use crate::error::TransportError;

pub struct TlsClient {
    connector: TlsConnector,
}

impl TlsClient {
    /// Создаёт TLS-клиент с системными корневыми сертификатами.
    pub fn new() -> Result<Self, TransportError> {
        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
        })
    }

    /// Устанавливает TLS-соединение с удалённым сервером.
    /// Возвращает готовый TLS-стрим поверх TCP.
    pub async fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Result<TlsStream<TcpStream>, TransportError> {
        let addr = format!("{}:{}", host, port);
        let tcp = TcpStream::connect(&addr).await?;

        let server_name = ServerName::try_from(host.to_string())
            .map_err(|_| TransportError::InvalidHost(host.to_string()))?;

        let tls_stream = self.connector.connect(server_name, tcp).await?;
        Ok(tls_stream)
    }
}
