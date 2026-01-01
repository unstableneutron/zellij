use anyhow::Result;
use std::env;
use std::time::{Duration, Instant};
use wtransport::{ClientConfig, Endpoint};

// Connection migration test:
// QUIC connections are identified by Connection IDs, not IP:port tuples.
// This means a connection can survive network changes (WiFi → mobile).
//
// To test migration:
// 1. Start echo server on a remote machine (e.g., vn3 or sjc3)
// 2. Start this client with MIGRATION_TEST=1
// 3. While running, switch networks (WiFi off → mobile on)
// 4. If connection survives and RTTs continue, migration worked!
//
// Note: WebTransport/wtransport uses quinn underneath, which supports
// connection migration. The client will automatically use the new path.

struct RttStats {
    samples: Vec<Duration>,
}

impl RttStats {
    fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    fn add(&mut self, rtt: Duration) {
        self.samples.push(rtt);
    }

    fn summary(&self) -> String {
        if self.samples.is_empty() {
            return "no samples".to_string();
        }

        let mut sorted: Vec<_> = self.samples.iter().copied().collect();
        sorted.sort();

        let min = sorted.first().unwrap();
        let max = sorted.last().unwrap();
        let median = sorted[sorted.len() / 2];
        let p95 = sorted[(sorted.len() as f64 * 0.95) as usize];
        let avg: Duration = self.samples.iter().sum::<Duration>() / self.samples.len() as u32;

        format!(
            "n={}, min={:.2}ms, avg={:.2}ms, median={:.2}ms, p95={:.2}ms, max={:.2}ms",
            self.samples.len(),
            min.as_secs_f64() * 1000.0,
            avg.as_secs_f64() * 1000.0,
            median.as_secs_f64() * 1000.0,
            p95.as_secs_f64() * 1000.0,
            max.as_secs_f64() * 1000.0,
        )
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let server_url =
        env::var("SERVER_URL").unwrap_or_else(|_| "https://127.0.0.1:4433".to_string());
    let ping_count: usize = env::var("PING_COUNT")
        .unwrap_or_else(|_| "100".to_string())
        .parse()?;
    let payload_size: usize = env::var("PAYLOAD_SIZE")
        .unwrap_or_else(|_| "64".to_string())
        .parse()?;
    let ping_interval_ms: u64 = env::var("PING_INTERVAL_MS")
        .unwrap_or_else(|_| "0".to_string())
        .parse()?;
    let migration_test = env::var("MIGRATION_TEST").is_ok();

    log::info!("Connecting to {}...", server_url);
    log::info!(
        "Will send {} pings with {} byte payload",
        ping_count,
        payload_size
    );
    if migration_test {
        log::info!(
            "MIGRATION TEST MODE: Switch networks during the test to verify connection survives"
        );
        log::info!("The test will run slowly (1 ping/sec) to give you time to switch");
    }

    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();

    let connect_start = Instant::now();
    let connection = Endpoint::client(config)?.connect(&server_url).await?;
    let connect_time = connect_start.elapsed();
    log::info!("Connected in {:.2}ms", connect_time.as_secs_f64() * 1000.0);

    let (mut send, mut recv) = connection.open_bi().await?.await?;
    log::info!("Bidirectional stream opened");

    let mut rtt_stats = RttStats::new();
    let mut buf = vec![0u8; payload_size + 64];

    let payload: Vec<u8> = (0..payload_size).map(|i| (i % 256) as u8).collect();

    for i in 0..ping_count {
        let msg = format!("PING:{:08}:", i);
        let mut full_msg = msg.into_bytes();
        full_msg.extend_from_slice(&payload);

        let start = Instant::now();
        send.write_all(&full_msg).await?;

        let n = recv.read(&mut buf).await?.unwrap_or(0);
        let rtt = start.elapsed();

        if n != full_msg.len() || &buf[..n] != &full_msg[..] {
            log::warn!(
                "Mismatch at ping {}: sent {} bytes, got {} bytes",
                i,
                full_msg.len(),
                n
            );
        }

        rtt_stats.add(rtt);

        if migration_test || (i + 1) % 10 == 0 || i == ping_count - 1 {
            log::info!(
                "Ping {}/{}: RTT = {:.2}ms",
                i + 1,
                ping_count,
                rtt.as_secs_f64() * 1000.0
            );
        }

        if migration_test {
            tokio::time::sleep(Duration::from_secs(1)).await;
        } else if ping_interval_ms > 0 {
            tokio::time::sleep(Duration::from_millis(ping_interval_ms)).await;
        }
    }

    send.write_all(b"QUIT").await?;
    let n = recv.read(&mut buf).await?.unwrap_or(0);
    if &buf[..n] == b"BYE" {
        log::info!("Server acknowledged quit");
    }

    println!("\n=== QUIC Echo Test Results ===");
    println!("Server:       {}", server_url);
    println!("Connect time: {:.2}ms", connect_time.as_secs_f64() * 1000.0);
    println!("Payload size: {} bytes", payload_size);
    println!("RTT stats:    {}", rtt_stats.summary());
    println!("==============================\n");

    Ok(())
}
