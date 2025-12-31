use anyhow::Result;
use std::env;

use wtransport::{Endpoint, Identity, ServerConfig};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let listen_addr = env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:4433".to_string())
        .parse()?;

    let identity = Identity::self_signed(["localhost", "quic-echo"])
        .map_err(|e| anyhow::anyhow!("failed to create identity: {}", e))?;

    let config = ServerConfig::builder()
        .with_bind_address(listen_addr)
        .with_identity(identity)
        .build();

    let server = Endpoint::server(config)?;
    log::info!("QUIC echo server listening on {}", listen_addr);
    log::info!("Waiting for connections...");

    loop {
        let incoming = server.accept().await;
        let session_request = incoming.await?;
        log::info!("Connection from {}", session_request.authority());

        let connection = session_request.accept().await?;
        log::info!("Session established");

        tokio::spawn(async move {
            if let Err(e) = handle_connection(connection).await {
                log::error!("Connection error: {}", e);
            }
        });
    }
}

async fn handle_connection(connection: wtransport::Connection) -> Result<()> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    log::info!("Bidirectional stream opened");

    let mut buf = [0u8; 1024];
    let mut message_count = 0u64;

    loop {
        let n = match recv.read(&mut buf).await? {
            Some(n) => n,
            None => {
                log::info!("Stream closed after {} messages", message_count);
                break;
            }
        };

        message_count += 1;
        let received = &buf[..n];
        
        if received.starts_with(b"PING:") {
            log::debug!("Echo {} bytes (msg #{})", n, message_count);
            send.write_all(received).await?;
        } else if received == b"STATS" {
            let stats = format!("STATS:messages={}", message_count);
            send.write_all(stats.as_bytes()).await?;
        } else if received == b"QUIT" {
            log::info!("Client requested quit after {} messages", message_count);
            send.write_all(b"BYE").await?;
            break;
        } else {
            send.write_all(received).await?;
        }
    }

    Ok(())
}
