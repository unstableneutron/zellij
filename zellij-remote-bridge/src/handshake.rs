use anyhow::Result;
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use zellij_remote_protocol::{
    stream_envelope, Capabilities, ClientHello, ControllerLease, ControllerPolicy, ProtocolVersion,
    ServerHello, SessionState, StreamEnvelope,
};

use crate::framing::{decode_envelope, encode_envelope, DecodeResult};

const DEFAULT_SNAPSHOT_INTERVAL_MS: u32 = 5000;

#[derive(Debug)]
pub struct HandshakeResult {
    pub client_hello: ClientHello,
    pub server_hello: ServerHello,
    pub client_id: u64,
}

pub async fn run_handshake<R, W>(
    mut reader: R,
    mut writer: W,
    session_name: String,
    client_id: u64,
) -> Result<HandshakeResult>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = BytesMut::new();

    loop {
        let mut chunk = [0u8; 1024];
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            anyhow::bail!("connection closed during handshake");
        }
        buffer.extend_from_slice(&chunk[..n]);

        match decode_envelope(&mut buffer)? {
            DecodeResult::Complete(envelope) => {
                match envelope.msg {
                    Some(stream_envelope::Msg::ClientHello(client_hello)) => {
                        log::info!("Received ClientHello from {}", client_hello.client_name);

                        let server_hello = build_server_hello(&client_hello, &session_name, client_id);
                        let response = StreamEnvelope {
                            msg: Some(stream_envelope::Msg::ServerHello(server_hello.clone())),
                        };
                        let encoded = encode_envelope(&response)?;
                        writer.write_all(&encoded).await?;

                        log::info!("Sent ServerHello, handshake complete");

                        return Ok(HandshakeResult {
                            client_hello,
                            server_hello,
                            client_id,
                        });
                    }
                    _ => {
                        anyhow::bail!("expected ClientHello, got other message");
                    }
                }
            }
            DecodeResult::Incomplete => {
                continue;
            }
        }
    }
}

pub fn build_server_hello(client_hello: &ClientHello, session_name: &str, client_id: u64) -> ServerHello {
    let negotiated_caps = Capabilities {
        supports_datagrams: client_hello
            .capabilities
            .as_ref()
            .map(|c| c.supports_datagrams)
            .unwrap_or(false),
        max_datagram_bytes: zellij_remote_protocol::DEFAULT_MAX_DATAGRAM_BYTES,
        supports_style_dictionary: true,
        supports_styled_underlines: false,
        supports_prediction: true,
        supports_images: false,
        supports_clipboard: false,
        supports_hyperlinks: false,
    };

    ServerHello {
        negotiated_version: Some(ProtocolVersion {
            major: zellij_remote_protocol::ZRP_VERSION_MAJOR,
            minor: zellij_remote_protocol::ZRP_VERSION_MINOR,
        }),
        negotiated_capabilities: Some(negotiated_caps),
        client_id,
        session_name: session_name.to_string(),
        session_state: SessionState::Running.into(),
        lease: Some(ControllerLease {
            lease_id: 0,
            owner_client_id: 0,
            policy: ControllerPolicy::LastWriterWins.into(),
            current_size: None,
            remaining_ms: 0,
            duration_ms: 30000,
        }),
        resume_token: vec![],
        snapshot_interval_ms: DEFAULT_SNAPSHOT_INTERVAL_MS,
        max_inflight_inputs: 256,
        render_window: zellij_remote_protocol::DEFAULT_RENDER_WINDOW,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn make_client_hello() -> ClientHello {
        ClientHello {
            version: Some(ProtocolVersion { major: 1, minor: 0 }),
            capabilities: Some(Capabilities {
                supports_datagrams: true,
                max_datagram_bytes: 1200,
                supports_style_dictionary: true,
                supports_styled_underlines: false,
                supports_prediction: true,
                supports_images: false,
                supports_clipboard: false,
                supports_hyperlinks: false,
            }),
            client_name: "test-client".to_string(),
            bearer_token: vec![],
            resume_token: vec![],
        }
    }

    #[tokio::test]
    async fn test_handshake_success() {
        let (client_stream, server_stream) = duplex(4096);
        let (client_read, mut client_write) = tokio::io::split(client_stream);
        let (server_read, server_write) = tokio::io::split(server_stream);

        // Spawn server handshake
        let server_handle = tokio::spawn(async move {
            run_handshake(server_read, server_write, "test-session".to_string(), 42).await
        });

        // Client sends ClientHello
        let client_hello = make_client_hello();
        let envelope = StreamEnvelope {
            msg: Some(stream_envelope::Msg::ClientHello(client_hello.clone())),
        };
        let encoded = encode_envelope(&envelope).unwrap();
        client_write.write_all(&encoded).await.unwrap();

        // Client reads ServerHello
        let mut client_read = client_read;
        let mut buffer = BytesMut::new();
        let mut chunk = [0u8; 1024];
        let n = client_read.read(&mut chunk).await.unwrap();
        buffer.extend_from_slice(&chunk[..n]);

        match decode_envelope(&mut buffer).unwrap() {
            DecodeResult::Complete(response) => {
                match response.msg {
                    Some(stream_envelope::Msg::ServerHello(hello)) => {
                        assert_eq!(hello.client_id, 42);
                        assert_eq!(hello.session_name, "test-session");
                        assert!(hello.negotiated_capabilities.as_ref().unwrap().supports_datagrams);
                    }
                    _ => panic!("expected ServerHello"),
                }
            }
            DecodeResult::Incomplete => panic!("expected complete response"),
        }

        // Verify server result
        let result = server_handle.await.unwrap().unwrap();
        assert_eq!(result.client_hello.client_name, "test-client");
        assert_eq!(result.client_id, 42);
    }

    #[tokio::test]
    async fn test_handshake_datagrams_disabled() {
        let (client_stream, server_stream) = duplex(4096);
        let (client_read, mut client_write) = tokio::io::split(client_stream);
        let (server_read, server_write) = tokio::io::split(server_stream);

        let server_handle = tokio::spawn(async move {
            run_handshake(server_read, server_write, "test".to_string(), 1).await
        });

        // Client with datagrams disabled
        let mut client_hello = make_client_hello();
        client_hello.capabilities.as_mut().unwrap().supports_datagrams = false;
        
        let envelope = StreamEnvelope {
            msg: Some(stream_envelope::Msg::ClientHello(client_hello)),
        };
        let encoded = encode_envelope(&envelope).unwrap();
        client_write.write_all(&encoded).await.unwrap();

        // Read response
        let mut client_read = client_read;
        let mut buffer = BytesMut::new();
        let mut chunk = [0u8; 1024];
        let n = client_read.read(&mut chunk).await.unwrap();
        buffer.extend_from_slice(&chunk[..n]);

        match decode_envelope(&mut buffer).unwrap() {
            DecodeResult::Complete(response) => {
                match response.msg {
                    Some(stream_envelope::Msg::ServerHello(hello)) => {
                        // Server should honor client's datagram preference
                        assert!(!hello.negotiated_capabilities.as_ref().unwrap().supports_datagrams);
                    }
                    _ => panic!("expected ServerHello"),
                }
            }
            DecodeResult::Incomplete => panic!("expected complete response"),
        }

        server_handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_handshake_connection_closed() {
        let (client_stream, server_stream) = duplex(4096);
        let (server_read, server_write) = tokio::io::split(server_stream);

        // Drop entire client stream to simulate connection close
        drop(client_stream);

        let result = run_handshake(server_read, server_write, "test".to_string(), 1).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("connection closed"));
    }

    #[tokio::test]
    async fn test_handshake_wrong_first_message() {
        let (client_stream, server_stream) = duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client_stream);
        let (server_read, server_write) = tokio::io::split(server_stream);

        // Send ServerHello instead of ClientHello
        let wrong_message = StreamEnvelope {
            msg: Some(stream_envelope::Msg::ServerHello(ServerHello::default())),
        };
        let encoded = encode_envelope(&wrong_message).unwrap();
        client_write.write_all(&encoded).await.unwrap();

        let result = run_handshake(server_read, server_write, "test".to_string(), 1).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected ClientHello"));
    }

    #[tokio::test]
    async fn test_handshake_partial_message() {
        let (client_stream, server_stream) = duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client_stream);
        let (server_read, server_write) = tokio::io::split(server_stream);

        let server_handle = tokio::spawn(async move {
            run_handshake(server_read, server_write, "test".to_string(), 1).await
        });

        // Send partial message first
        let client_hello = make_client_hello();
        let envelope = StreamEnvelope {
            msg: Some(stream_envelope::Msg::ClientHello(client_hello)),
        };
        let encoded = encode_envelope(&envelope).unwrap();

        // Send first half
        let mid = encoded.len() / 2;
        client_write.write_all(&encoded[..mid]).await.unwrap();
        
        // Small delay to let server process partial
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        
        // Send second half
        client_write.write_all(&encoded[mid..]).await.unwrap();

        // Should succeed
        let result = server_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_server_hello_required_fields() {
        let client_hello = make_client_hello();
        let hello = build_server_hello(&client_hello, "test-session", 123);

        assert!(hello.negotiated_version.is_some());
        assert!(hello.negotiated_capabilities.is_some());
        assert!(hello.lease.is_some());
        assert_eq!(hello.client_id, 123);
        assert_eq!(hello.session_name, "test-session");
        assert!(hello.snapshot_interval_ms > 0);
        assert!(hello.max_inflight_inputs > 0);
        assert!(hello.render_window > 0);
    }

    #[test]
    fn test_build_server_hello_no_client_capabilities() {
        let client_hello = ClientHello {
            version: Some(ProtocolVersion { major: 1, minor: 0 }),
            capabilities: None, // No capabilities
            client_name: "minimal".to_string(),
            bearer_token: vec![],
            resume_token: vec![],
        };

        let hello = build_server_hello(&client_hello, "test", 1);
        
        // Should default to no datagrams
        assert!(!hello.negotiated_capabilities.as_ref().unwrap().supports_datagrams);
    }
}
