// Overlay module - layer-shell bar + dialogs for the space manager overlay.
pub mod bar;
pub mod dialog_utils;
pub mod ipc_helpers;
pub mod model;
pub mod settings_dialog;
pub mod template_dialogs;
pub mod theme;
pub mod ui_components;
pub mod window_utils;

// Public overlay API used by the daemon.
pub use bar::{OverlayHandle, OverlayMsg};
pub use model::SpaceButton;
