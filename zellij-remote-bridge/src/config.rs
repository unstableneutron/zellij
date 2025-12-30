use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub listen_addr: SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub session_name: String,
    pub max_clients_per_session: usize,
    pub render_window: u32,
    pub controller_lease_duration_ms: u32,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:4433".parse().unwrap(),
            tls_cert: None,
            tls_key: None,
            session_name: "default".to_string(),
            max_clients_per_session: 10,
            render_window: 4,
            controller_lease_duration_ms: 30000,
        }
    }
}
