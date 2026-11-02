//! Typed Hyprland access layer (AF-4).
//!
//! Reads use Hyprland's command socket directly and deserialize only the fields
//! Space Manager needs. This keeps special workspaces (which have no numeric id)
//! and newly-added fields compatible with current Hyprland releases.
//!
//! Hyprland 0.56+ Lua configs reject the legacy dispatch and keyword strings
//! emitted by `hyprland-rs`, so writes use `hyprctl dispatch` / `hyprctl eval`
//! with Lua expressions instead.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::warn;

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

#[derive(Debug, Clone)]
pub enum WorkspaceTarget {
    Id(i32),
    Special(String),
}

#[derive(Deserialize)]
struct WorkspaceData {
    #[serde(default)]
    id: Option<i32>,
    name: String,
}

impl WorkspaceData {
    fn id(&self) -> i32 {
        self.id.unwrap_or(-1)
    }
}

#[derive(Deserialize)]
struct ClientData {
    address: String,
    class: String,
    title: String,
    pid: i64,
    at: (i32, i32),
    size: (i32, i32),
    workspace: WorkspaceData,
    #[serde(default)]
    monitor: Option<i128>,
}

impl From<ClientData> for ClientInfo {
    fn from(client: ClientData) -> Self {
        Self {
            address: client.address,
            class: client.class,
            title: client.title,
            pid: u32::try_from(client.pid).ok(),
            at: client.at,
            size: client.size,
            workspace_id: client.workspace.id(),
            workspace_name: client.workspace.name,
            monitor: client.monitor,
        }
    }
}

#[derive(Deserialize)]
struct MonitorData {
    id: i128,
    name: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(rename = "activeWorkspace")]
    active_workspace: WorkspaceData,
    focused: bool,
}

impl From<MonitorData> for MonitorInfo {
    fn from(monitor: MonitorData) -> Self {
        Self {
            id: monitor.id,
            name: monitor.name,
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
            active_workspace_id: monitor.active_workspace.id(),
            focused: monitor.focused,
        }
    }
}

pub async fn clients() -> Result<Vec<ClientInfo>> {
    let clients: Vec<ClientData> = request_json("clients").await?;
    Ok(clients.into_iter().map(ClientInfo::from).collect())
}

pub async fn monitors() -> Result<Vec<MonitorInfo>> {
    let monitors: Vec<MonitorData> = request_json("monitors").await?;
    Ok(monitors.into_iter().map(MonitorInfo::from).collect())
}

pub async fn active_workspace_id() -> Result<i32> {
    let workspace: WorkspaceData = request_json("activeworkspace").await?;
    Ok(workspace.id())
}

pub async fn cursor_position() -> Result<(i32, i32)> {
    #[derive(Deserialize)]
    struct CursorData {
        x: i32,
        y: i32,
    }

    let cursor: CursorData = request_json("cursorpos").await?;
    Ok((cursor.x, cursor.y))
}

pub async fn active_window() -> Result<Option<ClientInfo>> {
    let value: serde_json::Value = request_json("activewindow").await?;
    if value.get("address").is_none() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_value::<ClientData>(value)
            .context("failed to parse active window")?
            .into(),
    ))
}

fn socket_path(name: &str) -> Result<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is not set; cannot locate Hyprland IPC")?;
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE is not set; cannot locate Hyprland IPC")?;
    Ok(PathBuf::from(runtime_dir)
        .join("hypr")
        .join(instance)
        .join(name))
}

pub fn event_socket_path() -> Result<PathBuf> {
    socket_path(".socket2.sock")
}

async fn request_json<T: DeserializeOwned>(command: &str) -> Result<T> {
    let path = socket_path(".socket.sock")?;
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("failed to connect to {}", path.display()))?;
    stream
        .write_all(format!("j/{command}").as_bytes())
        .await
        .with_context(|| format!("failed to request Hyprland {command}"))?;
    stream.shutdown().await?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .with_context(|| format!("failed to read Hyprland {command} response"))?;
    serde_json::from_slice(&response).with_context(|| {
        format!(
            "failed to parse Hyprland {command} response: {}",
            String::from_utf8_lossy(&response)
        )
    })
}

/// Move a window (by address) to a workspace silently (no focus change).
pub async fn move_to_workspace_silent(target: WorkspaceTarget, addr: &str) -> Result<()> {
    dispatch(&lua_move_to_workspace(&target, addr)).await
}

/// Close a window by address.
pub async fn close_window(addr: &str) -> Result<()> {
    dispatch(&lua_close_window(addr)).await
}

/// Focus a window by address.
pub async fn focus_window(addr: &str) -> Result<()> {
    dispatch(&lua_focus_window(addr)).await
}

/// Resize a window to an exact pixel size (dialogs).
pub async fn resize_window_exact(addr: &str, width: i32, height: i32) -> Result<()> {
    dispatch(&lua_resize_window(addr, width, height)).await
}

/// Apply a one-shot float+center window rule matched by title (dialogs).
pub async fn float_center_by_title(title: &str) -> Result<()> {
    eval(&lua_float_center_rule(title)).await?;
    Ok(())
}

/// Set a Hyprland config option via `hl.config` (Lua config manager).
pub async fn keyword_set(key: &str, value: &str) -> Result<()> {
    eval(&keyword_to_lua_config(key, value)?).await?;
    Ok(())
}

/// Read an integer-valued Hyprland option (`hyprctl getoption section.option`).
pub async fn get_option_int(key: &str) -> Result<i64> {
    let dotted = key.replace(':', ".");
    let out = run_hyprctl(&["getoption", &dotted]).await?;
    parse_option_int(&out, key)
}

// ---------------------------------------------------------------------------
// Lua expression builders (also used by the GTK-thread dialog helpers)
// ---------------------------------------------------------------------------

/// Quote a string as a Lua `"..."` literal.
pub fn lua_quote(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Hyprland window selector for an address from `hyprctl clients -j`.
pub fn window_selector(addr: &str) -> String {
    let addr = addr.trim();
    if addr.starts_with("address:") {
        addr.to_string()
    } else {
        format!("address:{addr}")
    }
}

pub fn lua_move_to_workspace(target: &WorkspaceTarget, addr: &str) -> String {
    let workspace = match target {
        WorkspaceTarget::Id(id) => id.to_string(),
        WorkspaceTarget::Special(name) => lua_quote(&format!("special:{name}")),
    };
    format!(
        "hl.dsp.window.move({{ workspace = {workspace}, follow = false, window = {} }})",
        lua_quote(&window_selector(addr))
    )
}

pub fn lua_close_window(addr: &str) -> String {
    format!(
        "hl.dsp.window.close({{ window = {} }})",
        lua_quote(&window_selector(addr))
    )
}

pub fn lua_focus_window(addr: &str) -> String {
    format!(
        "hl.dsp.focus({{ window = {} }})",
        lua_quote(&window_selector(addr))
    )
}

pub fn lua_resize_window(addr: &str, width: i32, height: i32) -> String {
    format!(
        "hl.dsp.window.resize({{ x = {width}, y = {height}, window = {} }})",
        lua_quote(&window_selector(addr))
    )
}

pub fn lua_float_center_rule(title: &str) -> String {
    format!(
        "hl.window_rule({{ match = {{ title = {} }}, float = true, center = true }})",
        lua_quote(title)
    )
}

/// `input:follow_mouse` / `input.follow_mouse` → `hl.config({ input = { follow_mouse = 0 } })`.
pub fn keyword_to_lua_config(key: &str, value: &str) -> Result<String> {
    let parts: Vec<&str> = key.split([':', '.']).filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        bail!("keyword {key} is not a section.option path");
    }
    let lua_value = lua_scalar(value);
    let mut expr = format!("{} = {}", parts[parts.len() - 1], lua_value);
    for part in parts.iter().rev().skip(1) {
        expr = format!("{part} = {{ {expr} }}");
    }
    Ok(format!("hl.config({{ {expr} }})"))
}

fn lua_scalar(value: &str) -> String {
    if value == "true" || value == "false" {
        return value.to_string();
    }
    if value.parse::<i64>().is_ok() {
        return value.to_string();
    }
    if let Ok(f) = value.parse::<f64>() {
        if !f.is_nan() && f.is_finite() {
            return value.to_string();
        }
    }
    lua_quote(value)
}

pub fn parse_option_int(out: &str, key: &str) -> Result<i64> {
    for line in out.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("int:") {
            return rest
                .trim()
                .parse::<i64>()
                .with_context(|| format!("option {key} int parse failed: {out}"));
        }
        if let Some(rest) = line.strip_prefix("float:") {
            let f: f64 = rest
                .trim()
                .parse()
                .with_context(|| format!("option {key} float parse failed: {out}"))?;
            return Ok(f as i64);
        }
        if let Some(rest) = line.strip_prefix("str:") {
            return rest
                .trim()
                .parse::<i64>()
                .with_context(|| format!("option {key} is not an integer: {out}"));
        }
    }
    bail!("option {key} is not an integer: {out}");
}

// ---------------------------------------------------------------------------
// hyprctl
// ---------------------------------------------------------------------------

async fn dispatch(expr: &str) -> Result<()> {
    run_hyprctl(&["dispatch", expr]).await?;
    Ok(())
}

async fn eval(expr: &str) -> Result<String> {
    run_hyprctl(&["eval", expr]).await
}

async fn run_hyprctl(args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("hyprctl")
        .args(args)
        .output()
        .await
        .with_context(|| format!("failed to spawn hyprctl {args:?}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    if !output.status.success()
        || combined.contains("error:")
        || combined.contains("')' expected")
        || combined.contains("can't work with non-legacy")
    {
        bail!("hyprctl {args:?}: {}", combined.trim());
    }
    if combined.to_ascii_lowercase().contains("warning:") {
        warn!("hyprctl {args:?}: {}", combined.trim());
    }
    Ok(stdout.trim().to_string())
}

/// Blocking hyprctl for GTK-thread helpers (dialogs / Drop fallbacks).
pub fn run_hyprctl_blocking(args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("hyprctl")
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn hyprctl {args:?}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    if !output.status.success()
        || combined.contains("error:")
        || combined.contains("')' expected")
        || combined.contains("can't work with non-legacy")
    {
        bail!("hyprctl {args:?}: {}", combined.trim());
    }
    Ok(stdout.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_quote_escapes() {
        assert_eq!(lua_quote(r#"foo"bar"#), r#""foo\"bar""#);
        assert_eq!(lua_quote("a\\b"), r#""a\\b""#);
    }

    #[test]
    fn test_window_selector_prefixes_once() {
        assert_eq!(window_selector("0xabc"), "address:0xabc");
        assert_eq!(window_selector("address:0xabc"), "address:0xabc");
        assert_eq!(window_selector("  0xabc  "), "address:0xabc");
    }

    #[test]
    fn test_lua_move_id_and_special() {
        let id = lua_move_to_workspace(&WorkspaceTarget::Id(2), "0x55");
        assert_eq!(
            id,
            r#"hl.dsp.window.move({ workspace = 2, follow = false, window = "address:0x55" })"#
        );
        let spec =
            lua_move_to_workspace(&WorkspaceTarget::Special("spaces".into()), "address:0x55");
        assert_eq!(
            spec,
            r#"hl.dsp.window.move({ workspace = "special:spaces", follow = false, window = "address:0x55" })"#
        );
    }

    #[test]
    fn test_lua_close_and_focus() {
        assert_eq!(
            lua_close_window("0x1"),
            r#"hl.dsp.window.close({ window = "address:0x1" })"#
        );
        assert_eq!(
            lua_focus_window("0x1"),
            r#"hl.dsp.focus({ window = "address:0x1" })"#
        );
    }

    #[test]
    fn test_keyword_to_lua_config() {
        assert_eq!(
            keyword_to_lua_config("input:follow_mouse", "0").unwrap(),
            "hl.config({ input = { follow_mouse = 0 } })"
        );
        assert_eq!(
            keyword_to_lua_config("input.follow_mouse", "1").unwrap(),
            "hl.config({ input = { follow_mouse = 1 } })"
        );
        assert_eq!(
            keyword_to_lua_config("misc.disable_hyprland_logo", "true").unwrap(),
            "hl.config({ misc = { disable_hyprland_logo = true } })"
        );
    }

    #[test]
    fn test_parse_option_int() {
        assert_eq!(
            parse_option_int("int: 1\nset: true\n", "input.follow_mouse").unwrap(),
            1
        );
        assert_eq!(parse_option_int("float: 1.0\n", "x").unwrap(), 1);
        assert!(parse_option_int("str: intl\n", "input.kb_variant").is_err());
    }

    #[test]
    fn test_client_json_allows_special_workspace_without_id() {
        let client: ClientData = serde_json::from_str(
            r#"{
                "address":"0xabc", "class":"brave", "title":"Work", "pid":42,
                "at":[10,20], "size":[800,600],
                "workspace":{"address":"special:spaces","type":"special","name":"special:spaces"},
                "monitor":1, "newHyprlandField":"ignored"
            }"#,
        )
        .unwrap();
        let client = ClientInfo::from(client);
        assert_eq!(client.workspace_id, -1);
        assert_eq!(client.workspace_name, "special:spaces");
        assert_eq!(client.at, (10, 20));
        assert_eq!(client.size, (800, 600));
    }
}
