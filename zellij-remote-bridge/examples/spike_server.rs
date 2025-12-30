use anyhow::Result;
use zellij_remote_bridge::{BridgeConfig, RemoteBridge};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let config = BridgeConfig::default();
    let bridge = RemoteBridge::new(config);

    println!("Starting spike server on 127.0.0.1:4433");
    bridge.run().await
}
