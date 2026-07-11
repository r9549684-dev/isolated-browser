use std::sync::Arc;
use rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use rustls::pki_types::ServerName;
use crate::error::TransportError;

pub struct TlsClient {
    connector: TlsConnector,
    sni_pool: Vec<String>,
    current_sni_idx: usize,
}

impl TlsClient {
    pub fn new(sni_pool: Vec<String>) -> Result<Self, TransportError> {
        if sni_pool.is_empty() {
            return Err(TransportError::Protocol("SNI pool is empty".into()));
        }

        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
            sni_pool,
            current_sni_idx: 0,
        })
    }

    pub fn rotate_sni(&mut self) {
        self.current_sni_idx = (self.current_sni_idx + 1) % self.sni_pool.len();
    }

    pub fn current_sni(&self) -> &str {
        &self.sni_pool[self.current_sni_idx]
    }

    pub async fn connect(
        &mut self,
        host: &str,
        port: u16,
    ) -> Result<TlsStream<TcpStream>, TransportError> {
        let addr = format!("{}:{}", host, port);
        let tcp = TcpStream::connect(&addr).await?;

        let sni = self.current_sni().to_string();
        let server_name = ServerName::try_from(sni.clone())
            .map_err(|_| TransportError::InvalidHost(sni))?;

        let tls_stream = self.connector.connect(server_name, tcp).await?;
        self.rotate_sni();
        Ok(tls_stream)
    }
}
