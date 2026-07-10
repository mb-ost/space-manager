//! File logging + panic hook (AF-5, AC-e).
//!
//! Writes daily-rotating logs to `~/.space-manager/logs/space-manager.log` via a
//! non-blocking `tracing-appender` writer, keeps a stdout layer (harmless when
//! detached), and installs a panic hook that records the panic payload,
//! location, and backtrace at `error!` before default behavior.

use std::path::PathBuf;

use tracing::error;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

fn log_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".space-manager").join("logs")
}

/// Initialize logging. Returns a guard that must be kept alive for the process
/// lifetime so the non-blocking writer is flushed on shutdown.
pub fn init() -> WorkerGuard {
    let dir = log_dir();
    // Best-effort dir creation; if it fails we still get stdout logging.
    let _ = std::fs::create_dir_all(&dir);

    let file_appender = tracing_appender::rolling::daily(&dir, "space-manager.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(non_blocking);
    let stdout_layer = tracing_subscriber::fmt::layer();

    // If a global subscriber is already set (e.g. in tests), don't panic.
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stdout_layer)
        .try_init();

    install_panic_hook();

    guard
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();
        error!(
            "PANIC at {}: {}\nbacktrace:\n{}",
            location, payload, backtrace
        );
        default_hook(info);
    }));
}
