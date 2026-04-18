use crate::types::{Command, Response};
use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::debug;

pub fn get_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("space-manager.sock")
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
