use anyhow::{Context, Result};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tracing::debug;

pub struct ProcessLauncher {
    pending_spawns: std::sync::Arc<tokio::sync::RwLock<Vec<PendingSpawn>>>,
}

#[derive(Debug, Clone)]
pub struct PendingSpawn {
    pub command: String,
    pub spawn_time: u64,
    pub pid: Option<u32>,
}

impl ProcessLauncher {
    pub fn new() -> Self {
        Self {
            pending_spawns: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    pub async fn spawn(&self, command: String) -> Result<u32> {
        debug!("Spawning command: {}", command);

        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to spawn process")?;

        let pid = child.id().context("Failed to get process PID")?;

        let spawn_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let pending = PendingSpawn {
            command,
            spawn_time,
            pid: Some(pid),
        };

        self.pending_spawns.write().await.push(pending);

        // Clean up old pending spawns (older than 10 seconds)
        self.cleanup_old_spawns().await;

        Ok(pid)
    }

    pub async fn match_window(
        &self,
        class: &str,
        pid: Option<u32>,
    ) -> Option<PendingSpawn> {
        let mut spawns = self.pending_spawns.write().await;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Try to match by PID first (within last 10 seconds)
        if let Some(window_pid) = pid {
            if let Some(pos) = spawns.iter().position(|s| {
                s.pid == Some(window_pid) && now - s.spawn_time < 10
            }) {
                return Some(spawns.remove(pos));
            }
        }

        // Fallback: match by class name or window title matching command (within 10 seconds)
        // This is more lenient to catch browsers that spawn multiple processes
        if let Some(pos) = spawns.iter().position(|s| {
            if now - s.spawn_time >= 10 {
                return false;
            }

            let cmd_lower = s.command.to_lowercase();
            let class_lower = class.to_lowercase();

            // Direct class name match
            if cmd_lower.contains(&class_lower) {
                return true;
            }

            // Browser-specific matching
            // Brave can have class names like "brave-browser", "Brave-browser", or "WebApp-*"
            if cmd_lower.contains("brave") {
                return class_lower.contains("brave") || class_lower.starts_with("webapp-");
            }

            if cmd_lower.contains("chromium") && class_lower.contains("chromium") {
                return true;
            }

            if cmd_lower.contains("firefox") && class_lower.contains("firefox") {
                return true;
            }

            false
        }) {
            return Some(spawns.remove(pos));
        }

        None
    }

    async fn cleanup_old_spawns(&self) {
        let mut spawns = self.pending_spawns.write().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        spawns.retain(|s| now - s.spawn_time < 10);
    }
}

impl Default for ProcessLauncher {
    fn default() -> Self {
        Self::new()
    }
}
