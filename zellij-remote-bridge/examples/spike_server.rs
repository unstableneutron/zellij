use anyhow::Result;
use zellij_remote_bridge::{BridgeConfig, RemoteBridge};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let listen_addr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:4433".to_string())
        .parse()
        .expect("Invalid LISTEN_ADDR");

    let config = BridgeConfig {
        listen_addr,
        ..Default::default()
    };
    let bridge = RemoteBridge::new(config);

    println!("Starting spike server on {}", listen_addr);
    bridge.run().await
}
