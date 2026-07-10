//! Typed async Hyprland access layer (AF-4).
//!
//! Every Hyprland read/write in the daemon goes through this module. It wraps the
//! `hyprland-rs` 0.4.0-beta.3 async API and exposes thin, local structs
//! (`ClientInfo`, `MonitorInfo`) so the rest of the codebase never depends on
//! `hyprland-rs` types directly.
//!
//! No raw `hyprctl` subprocess is used anywhere. If a capability is ever missing
//! from the crate, document it as an explicit exception here. As of the audit,
//! none is required.

use anyhow::Result;
use hyprland::data::{Client, Clients, CursorPosition, Monitors, Workspace};
use hyprland::dispatch::{Dispatch, DispatchType, WindowIdentifier, WorkspaceIdentifierWithSpecial};
use hyprland::keyword::{Keyword, OptionValue};
use hyprland::shared::{Address, HyprData, HyprDataActive, HyprDataActiveOptional};

/// A live Hyprland client (window), projected into local types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub address: String,
    pub class: String,
    pub title: String,
    pub pid: Option<u32>,
    pub at: (i32, i32),
    pub size: (i32, i32),
    pub workspace_id: i32,
    pub workspace_name: String,
    pub monitor: Option<i128>,
}

/// A live Hyprland monitor, projected into local types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub id: i128,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub active_workspace_id: i32,
    pub focused: bool,
}

/// Target workspace for a silent move (either a normal id or a special workspace).
#[derive(Debug, Clone)]
pub enum WorkspaceTarget {
    Id(i32),
    Special(String),
}

fn client_to_info(client: &Client) -> ClientInfo {
    let pid = if client.pid >= 0 {
        Some(client.pid as u32)
    } else {
        None
    };
    ClientInfo {
        address: client.address.to_string(),
        class: client.class.clone(),
        title: client.title.clone(),
        pid,
        at: (client.at.0 as i32, client.at.1 as i32),
        size: (client.size.0 as i32, client.size.1 as i32),
        workspace_id: client.workspace.id,
        workspace_name: client.workspace.name.clone(),
        monitor: client.monitor,
    }
}

/// Snapshot all live clients.
pub async fn clients() -> Result<Vec<ClientInfo>> {
    let clients = Clients::get_async().await?;
    Ok(clients.iter().map(client_to_info).collect())
}

/// Snapshot all monitors.
pub async fn monitors() -> Result<Vec<MonitorInfo>> {
    let monitors = Monitors::get_async().await?;
    Ok(monitors
        .iter()
        .map(|m| MonitorInfo {
            id: m.id,
            name: m.name.clone(),
            x: m.x,
            y: m.y,
            width: m.width as i32,
            height: m.height as i32,
            active_workspace_id: m.active_workspace.id,
            focused: m.focused,
        })
        .collect())
}

/// The id of the currently active workspace.
pub async fn active_workspace_id() -> Result<i32> {
    let ws = Workspace::get_active_async().await?;
    Ok(ws.id)
}

/// The current cursor position in global coordinates.
pub async fn cursor_position() -> Result<(i32, i32)> {
    let pos = CursorPosition::get_async().await?;
    Ok((pos.x as i32, pos.y as i32))
}

/// The currently focused window, if any.
pub async fn active_window() -> Result<Option<ClientInfo>> {
    let active = Client::get_active_async().await?;
    Ok(active.as_ref().map(client_to_info))
}

/// Move a window (by address) to a workspace silently (no focus change).
pub async fn move_to_workspace_silent(target: WorkspaceTarget, addr: &str) -> Result<()> {
    let identifier = WindowIdentifier::Address(Address::new(addr));
    match target {
        WorkspaceTarget::Id(id) => {
            Dispatch::call_async(DispatchType::MoveToWorkspaceSilent(
                WorkspaceIdentifierWithSpecial::Id(id),
                Some(identifier),
            ))
            .await?;
        }
        WorkspaceTarget::Special(name) => {
            Dispatch::call_async(DispatchType::MoveToWorkspaceSilent(
                WorkspaceIdentifierWithSpecial::Special(Some(name.as_str())),
                Some(identifier),
            ))
            .await?;
        }
    }
    Ok(())
}

/// Close a window by address.
pub async fn close_window(addr: &str) -> Result<()> {
    Dispatch::call_async(DispatchType::CloseWindow(WindowIdentifier::Address(
        Address::new(addr),
    )))
    .await?;
    Ok(())
}

/// Focus a window by address.
pub async fn focus_window(addr: &str) -> Result<()> {
    Dispatch::call_async(DispatchType::FocusWindow(WindowIdentifier::Address(
        Address::new(addr),
    )))
    .await?;
    Ok(())
}

/// Set a Hyprland keyword/option.
pub async fn keyword_set(key: &str, value: &str) -> Result<()> {
    Keyword::set_async(key.to_string(), value.to_string()).await?;
    Ok(())
}

/// Read an integer-valued Hyprland option.
pub async fn get_option_int(key: &str) -> Result<i64> {
    let kw = Keyword::get_async(key.to_string()).await?;
    match kw.value {
        OptionValue::Int(i) => Ok(i),
        OptionValue::Float(f) => Ok(f as i64),
        OptionValue::String(s) => s
            .parse::<i64>()
            .map_err(|_| anyhow::anyhow!("option {} is not an integer: {}", key, s)),
    }
}
