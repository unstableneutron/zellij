use anyhow::{Context, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;
use wtransport::{Endpoint, Identity, ServerConfig};

use crate::config::BridgeConfig;
use crate::handshake::run_handshake;

static CLIENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct RemoteBridge {
    config: BridgeConfig,
}

impl RemoteBridge {
    pub fn new(config: BridgeConfig) -> Self {
        Self { config }
    }

    pub async fn run(&self) -> Result<()> {
        self.run_with_shutdown(CancellationToken::new()).await
    }

    pub async fn run_with_shutdown(&self, shutdown: CancellationToken) -> Result<()> {
        let identity = self.build_identity().await?;

        let config = ServerConfig::builder()
            .with_bind_default(self.config.listen_addr.port())
            .with_identity(identity)
            .build();

        let server = Endpoint::server(config)?;

        log::info!(
            "WebTransport server listening on {}",
            self.config.listen_addr
        );

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    log::info!("Server shutdown requested");
                    return Ok(());
                }
                incoming = server.accept() => {
                    let session_request = incoming.await?;

                    log::info!("Incoming connection from {}", session_request.authority());

                    let connection = session_request.accept().await?;
                    let session_name = self.config.session_name.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(connection, session_name).await {
                            log::error!("Connection error: {}", e);
                        }
                    });
                }
            }
        }
    }

    async fn handle_connection(
        connection: wtransport::Connection,
        session_name: String,
    ) -> Result<()> {
        let (send, recv) = connection.accept_bi().await?;
        let client_id = CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

        let result = run_handshake(recv, send, session_name, client_id).await?;

        log::info!(
            "Handshake complete: client_id={}, client_name={}",
            result.client_id,
            result.client_hello.client_name
        );

        // For spike: just keep connection alive
        // Real implementation will proceed to main loop
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        Ok(())
    }

    async fn build_identity(&self) -> Result<Identity> {
        match (&self.config.tls_cert, &self.config.tls_key) {
            (Some(cert_path), Some(key_path)) => Identity::load_pemfiles(cert_path, key_path)
                .await
                .context("failed to load TLS certificate/key"),
            _ => {
                log::warn!("No TLS cert configured, generating self-signed certificate");
                Identity::self_signed(["localhost"])
                    .map_err(|e| anyhow::anyhow!("failed to create self-signed identity: {}", e))
            },
        }
    }
}
