// Overlay module - UI components for the space manager overlay
mod manager;
pub mod ipc_helpers;
pub mod ui_components;
pub mod theme;
pub mod window_utils;
pub mod dialog_utils;

// Re-export the main OverlayManager
pub use manager::OverlayManager;

