// Overlay module - UI components for the space manager overlay
pub mod dialog_utils;
pub mod ipc_helpers;
mod manager;
pub mod theme;
pub mod ui_components;
pub mod window_utils;

// Re-export the main OverlayManager
pub use manager::OverlayManager;
