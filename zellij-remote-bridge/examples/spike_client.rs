use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use prost::Message;
use wtransport::{ClientConfig, Endpoint};

use zellij_remote_protocol::{
    stream_envelope, Capabilities, ClientHello, ProtocolVersion, StreamEnvelope,
};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();

    let connection = Endpoint::client(config)?
        .connect("https://127.0.0.1:4433")
        .await
        .context("failed to connect to server")?;

    log::info!("Connected to server");

    let (mut send, mut recv) = connection.open_bi().await?.await?;

    let client_hello = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ClientHello(ClientHello {
            client_name: "spike-client".to_string(),
            version: Some(ProtocolVersion {
                major: zellij_remote_protocol::ZRP_VERSION_MAJOR,
                minor: zellij_remote_protocol::ZRP_VERSION_MINOR,
            }),
            capabilities: Some(Capabilities {
                supports_datagrams: false,
                max_datagram_bytes: zellij_remote_protocol::DEFAULT_MAX_DATAGRAM_BYTES,
                supports_style_dictionary: true,
                supports_styled_underlines: false,
                supports_prediction: true,
                supports_images: false,
                supports_clipboard: false,
                supports_hyperlinks: false,
            }),
            bearer_token: vec![],
            resume_token: vec![],
        })),
    };

    let encoded = encode_envelope(&client_hello)?;
    send.write_all(&encoded).await?;
    log::info!("Sent ClientHello");

    let mut buffer = BytesMut::new();
    loop {
        let mut chunk = [0u8; 1024];
        let n = recv.read(&mut chunk).await?.unwrap_or(0);
        if n == 0 {
            anyhow::bail!("connection closed before receiving ServerHello");
        }
        buffer.extend_from_slice(&chunk[..n]);

        if let Some(envelope) = decode_envelope(&mut buffer)? {
            match envelope.msg {
                Some(stream_envelope::Msg::ServerHello(hello)) => {
                    println!("=== ServerHello received ===");
                    println!("  Session name: {}", hello.session_name);
                    println!("  Client ID: {}", hello.client_id);
                    if let Some(v) = hello.negotiated_version {
                        println!("  Protocol version: {}.{}", v.major, v.minor);
                    }
                    if let Some(caps) = hello.negotiated_capabilities {
                        println!("  Capabilities:");
                        println!("    - datagrams: {}", caps.supports_datagrams);
                        println!("    - style_dictionary: {}", caps.supports_style_dictionary);
                        println!("    - prediction: {}", caps.supports_prediction);
                    }
                    println!("  Snapshot interval: {} ms", hello.snapshot_interval_ms);
                    println!("  Max inflight inputs: {}", hello.max_inflight_inputs);
                    println!("=== Handshake complete ===");
                    return Ok(());
                },
                _ => {
                    anyhow::bail!("expected ServerHello, got other message");
                },
            }
        }
    }
}

fn encode_envelope(envelope: &StreamEnvelope) -> Result<Vec<u8>> {
    let len = envelope.encoded_len();
    let mut buf = BytesMut::with_capacity(len + 5);
    prost::encoding::encode_varint(len as u64, &mut buf);
    envelope.encode(&mut buf)?;
    Ok(buf.to_vec())
}

fn decode_envelope(buf: &mut BytesMut) -> Result<Option<StreamEnvelope>> {
    if buf.is_empty() {
        return Ok(None);
    }

    let mut peek = &buf[..];
    let len = match prost::encoding::decode_varint(&mut peek) {
        Ok(len) => len as usize,
        Err(_) => {
            if buf.len() < 10 {
                return Ok(None);
            }
            anyhow::bail!("invalid varint in frame header");
        },
    };

    let varint_len = buf.len() - peek.len();
    let total_len = varint_len + len;

    if buf.len() < total_len {
        return Ok(None);
    }

    buf.advance(varint_len);
    let frame_data = buf.split_to(len);
    let envelope = StreamEnvelope::decode(&frame_data[..])?;
    Ok(Some(envelope))
}
