use crate::types::{Command, Response};
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::debug;

pub fn get_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("space-manager.sock")
}

/// Encode a value into a length-prefixed JSON frame (4-byte little-endian length
/// prefix followed by the JSON payload). Extracted for unit testing (AF-8).
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let data = serde_json::to_vec(value)?;
    let len = data.len() as u32;
    let mut frame = Vec::with_capacity(4 + data.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&data);
    Ok(frame)
}

/// Decode a length-prefixed JSON frame produced by [`encode_frame`]. Returns an
/// error (never panics) on a truncated frame or invalid payload.
pub fn decode_frame<T: DeserializeOwned>(frame: &[u8]) -> Result<T> {
    if frame.len() < 4 {
        anyhow::bail!("frame too short for length prefix");
    }
    let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    let payload = &frame[4..];
    if payload.len() < len {
        anyhow::bail!(
            "truncated frame: length prefix {} > payload {}",
            len,
            payload.len()
        );
    }
    let value = serde_json::from_slice(&payload[..len])?;
    Ok(value)
}

pub struct IpcServer {
    listener: UnixListener,
}

impl IpcServer {
    pub async fn new() -> Result<Self> {
        let socket_path = get_socket_path();

        // Remove old socket if it exists
        if socket_path.exists() {
            std::fs::remove_file(&socket_path).context("Failed to remove old socket")?;
        }

        let listener = UnixListener::bind(&socket_path).context("Failed to bind Unix socket")?;

        debug!("IPC server listening on {:?}", socket_path);

        Ok(Self { listener })
    }

    pub async fn accept(&self) -> Result<IpcConnection> {
        let (stream, _) = self.listener.accept().await?;
        Ok(IpcConnection { stream })
    }
}

pub struct IpcConnection {
    stream: UnixStream,
}

impl IpcConnection {
    pub async fn recv_command(&mut self) -> Result<Command> {
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        let mut buffer = vec![0u8; len];
        self.stream.read_exact(&mut buffer).await?;

        let command: Command = serde_json::from_slice(&buffer)?;
        Ok(command)
    }

    pub async fn send_response(&mut self, response: &Response) -> Result<()> {
        let data = serde_json::to_vec(response)?;
        let len = data.len() as u32;

        self.stream.write_all(&len.to_le_bytes()).await?;
        self.stream.write_all(&data).await?;
        self.stream.flush().await?;

        Ok(())
    }
}

pub struct IpcClient {
    stream: UnixStream,
}

impl IpcClient {
    pub async fn connect() -> Result<Self> {
        let socket_path = get_socket_path();
        let stream = UnixStream::connect(&socket_path)
            .await
            .context("Failed to connect to daemon. Is space-manager running?")?;

        Ok(Self { stream })
    }

    pub async fn send_command(&mut self, command: &Command) -> Result<()> {
        let data = serde_json::to_vec(command)?;
        let len = data.len() as u32;

        self.stream.write_all(&len.to_le_bytes()).await?;
        self.stream.write_all(&data).await?;
        self.stream.flush().await?;

        Ok(())
    }

    pub async fn recv_response(&mut self) -> Result<Response> {
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let len = u32::from_le_bytes(len_bytes) as usize;

        let mut buffer = vec![0u8; len];
        self.stream.read_exact(&mut buffer).await?;

        let response: Response = serde_json::from_slice(&buffer)?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Command, Response};

    #[test]
    fn test_frame_roundtrip_command() {
        let frame = encode_frame(&Command::Next).unwrap();
        let decoded: Command = decode_frame(&frame).unwrap();
        assert!(matches!(decoded, Command::Next));
    }

    #[test]
    fn test_frame_roundtrip_shutdown() {
        let frame = encode_frame(&Command::Shutdown).unwrap();
        let decoded: Command = decode_frame(&frame).unwrap();
        assert!(matches!(decoded, Command::Shutdown));
    }

    #[test]
    fn test_frame_roundtrip_response_windows() {
        let resp = Response::Windows(vec![crate::types::ManagedWindow::new("cmd".to_string())]);
        let frame = encode_frame(&resp).unwrap();
        let decoded: Response = decode_frame(&frame).unwrap();
        match decoded {
            Response::Windows(w) => assert_eq!(w.len(), 1),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_decode_truncated_frame() {
        // Length prefix claims 100 bytes but payload is empty.
        let mut frame = (100u32).to_le_bytes().to_vec();
        frame.extend_from_slice(b"short");
        let result: Result<Command> = decode_frame(&frame);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_garbage_payload() {
        let payload = b"not valid json";
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(payload);
        let result: Result<Command> = decode_frame(&frame);
        assert!(result.is_err());
    }

    #[test]
    fn test_backward_compat_legacy_command_json() {
        // JSON exactly as the current enum serializes it must still decode,
        // guarding wire-protocol stability across versions.
        let legacy = serde_json::to_vec(&serde_json::json!("Next")).unwrap();
        let mut frame = (legacy.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&legacy);
        let decoded: Command = decode_frame(&frame).unwrap();
        assert!(matches!(decoded, Command::Next));

        let legacy_switch = serde_json::to_vec(&serde_json::json!({"SwitchTo": 3})).unwrap();
        let mut frame2 = (legacy_switch.len() as u32).to_le_bytes().to_vec();
        frame2.extend_from_slice(&legacy_switch);
        let decoded2: Command = decode_frame(&frame2).unwrap();
        assert!(matches!(decoded2, Command::SwitchTo(3)));
    }
}
