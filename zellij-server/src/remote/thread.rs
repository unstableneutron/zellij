use std::net::SocketAddr;

use anyhow::Result;
use zellij_utils::channels::Receiver;
use zellij_utils::errors::{prelude::*, ErrorContext};
use zellij_utils::pane_size::Size;

use super::instruction::RemoteInstruction;
use super::manager::RemoteManager;

/// Configuration for the remote server
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    pub listen_addr: SocketAddr,
    pub session_name: String,
    pub initial_size: Size,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:4433".parse().unwrap(),
            session_name: "zellij".to_string(),
            initial_size: Size { cols: 80, rows: 24 },
        }
    }
}

/// Main entry point for the remote thread
pub fn remote_thread_main(
    receiver: Receiver<(RemoteInstruction, ErrorContext)>,
    config: RemoteConfig,
) -> Result<()> {
    log::info!(
        "Remote thread starting: listen_addr={}, session={}",
        config.listen_addr,
        config.session_name
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("remote-tokio")
        .build()
        .context("failed to create tokio runtime for remote thread")?;

    rt.block_on(async { run_remote_server(receiver, config).await })
}

async fn run_remote_server(
    receiver: Receiver<(RemoteInstruction, ErrorContext)>,
    config: RemoteConfig,
) -> Result<()> {
    let mut manager = RemoteManager::new(config.initial_size.cols, config.initial_size.rows);

    // TODO: Spawn WebTransport server here using zellij_remote_bridge
    // For now, just process instructions
    log::info!(
        "Remote server ready (WebTransport server not yet wired, listening for instructions)"
    );

    loop {
        match receiver.recv() {
            Ok((instruction, _err_ctx)) => {
                match handle_instruction(&mut manager, instruction).await {
                    Ok(should_exit) => {
                        if should_exit {
                            break;
                        }
                    },
                    Err(e) => {
                        log::error!("Error handling remote instruction: {}", e);
                    },
                }
            },
            Err(e) => {
                log::error!("Remote instruction channel closed: {}", e);
                break;
            },
        }
    }

    log::info!("Remote thread shutting down");
    Ok(())
}

/// Handle an instruction from the main thread.
/// Returns Ok(true) if the server should shut down, Ok(false) to continue.
async fn handle_instruction(
    manager: &mut RemoteManager,
    instruction: RemoteInstruction,
) -> Result<bool> {
    match instruction {
        RemoteInstruction::FrameReady {
            client_id,
            frame_store,
        } => {
            log::trace!(
                "Frame ready for client {}: state_id={}",
                client_id,
                frame_store.current_state_id()
            );
            // Update the manager's frame store
            // In future: compute delta and send to WebTransport clients
            let (cols, rows) = manager.dimensions();
            if frame_store.current_frame().cols != cols
                || frame_store.current_frame().rows.len() != rows
            {
                log::debug!(
                    "Frame size mismatch, updating manager dimensions: {}x{} -> {}x{}",
                    cols,
                    rows,
                    frame_store.current_frame().cols,
                    frame_store.current_frame().rows.len()
                );
            }
            // TODO: Send to connected WebTransport clients
        },
        RemoteInstruction::ClientResize { client_id, size } => {
            log::debug!(
                "Client {} resized: {}x{}",
                client_id,
                size.cols,
                size.rows
            );
            manager.resize(size.cols, size.rows);
        },
        RemoteInstruction::ClientConnected { client_id, size } => {
            log::info!(
                "Remote client {} connected: {}x{}",
                client_id,
                size.cols,
                size.rows
            );
            manager.add_client(client_id, size);
        },
        RemoteInstruction::ClientDisconnected { client_id } => {
            log::info!("Remote client {} disconnected", client_id);
            manager.remove_client(client_id);
        },
        RemoteInstruction::Shutdown => {
            log::info!("Remote thread received shutdown signal");
            return Ok(true);
        },
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_remote_core::FrameStore;

    #[test]
    fn test_remote_config_default() {
        let config = RemoteConfig::default();
        assert_eq!(config.listen_addr.port(), 4433);
        assert_eq!(config.session_name, "zellij");
        assert_eq!(config.initial_size.cols, 80);
        assert_eq!(config.initial_size.rows, 24);
    }

    #[tokio::test]
    async fn test_handle_client_connected() {
        let mut manager = RemoteManager::new(80, 24);
        let instruction = RemoteInstruction::ClientConnected {
            client_id: 1,
            size: Size { cols: 120, rows: 40 },
        };

        let should_exit = handle_instruction(&mut manager, instruction)
            .await
            .unwrap();
        assert!(!should_exit);
        assert!(manager.is_remote_client(1));
    }

    #[tokio::test]
    async fn test_handle_frame_ready() {
        let mut manager = RemoteManager::new(80, 24);
        let frame_store = FrameStore::new(80, 24);
        let instruction = RemoteInstruction::FrameReady {
            client_id: 1,
            frame_store,
        };

        let should_exit = handle_instruction(&mut manager, instruction)
            .await
            .unwrap();
        assert!(!should_exit);
    }

    #[tokio::test]
    async fn test_handle_shutdown() {
        let mut manager = RemoteManager::new(80, 24);
        let instruction = RemoteInstruction::Shutdown;

        let should_exit = handle_instruction(&mut manager, instruction)
            .await
            .unwrap();
        assert!(should_exit);
    }
}
