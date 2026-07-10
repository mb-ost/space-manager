//! Layer-shell overlay bar + the single GTK thread (AF-2, AF-3).
//!
//! The overlay is a `gtk4-layer-shell` surface, NOT a Hyprland client. There is
//! exactly one `gtk4::Application` and one window for the process lifetime; the
//! compositor re-maps the layer surface across output enable/disable, so the
//! window is never destroyed/recreated (this structurally fixes the old
//! second-`Application` failure and the pin-toggle inversion).
//!
//! The daemon (tokio) drives the bar by sending [`OverlayMsg`] over an
//! `async_channel`; the GTK side drains it on the glib main context via
//! `glib::spawn_future_local`. There is no polling tick and no label diffing.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Button, Orientation};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use tracing::{info, warn};

use super::dialog_utils;
use super::ipc_helpers;
use super::settings_dialog;
use super::template_dialogs;
use super::ui_components;
use crate::geometry::{Anchor, OVERLAY_HEIGHT};
use crate::overlay::model::SpaceButton;

const APP_ID: &str = "com.spacermanager.overlay";
const NAMESPACE: &str = "space-manager-overlay";
const CHANNEL_CAPACITY: usize = 64;

/// Messages sent from the daemon (tokio) to the overlay (GTK).
#[derive(Debug, Clone)]
pub enum OverlayMsg {
    /// Rebuild the space buttons.
    UpdateSpaces {
        spaces: Vec<SpaceButton>,
        current: usize,
    },
    /// Re-anchor the bar to a monitor-local position.
    Reposition {
        anchor: Anchor,
        margin_x: i32,
        margin_y: i32,
        width: i32,
        monitor: String,
    },
    /// Make the bar visible.
    Show,
    /// Hide the bar.
    Hide,
    /// Quit the GTK application (graceful daemon shutdown).
    Shutdown,
}

/// Daemon-side handle to the overlay. Cloneable; sends are non-blocking.
#[derive(Clone)]
pub struct OverlayHandle {
    tx: async_channel::Sender<OverlayMsg>,
}

impl OverlayHandle {
    /// Send a message to the overlay. Non-blocking `try_send`.
    ///
    /// The channel is bounded (capacity 64) and is not expected to fill in
    /// practice. If the GTK thread ever stalls and the channel fills, the
    /// intended target failure mode for `UpdateSpaces`/`Reposition` is
    /// latest-wins coalescing (a stale overlay is worse than a dropped
    /// intermediate frame). The first cut uses `try_send` with a `warn!` on
    /// full, which is acceptable per the refinement.
    pub fn send(&self, msg: OverlayMsg) {
        if let Err(e) = self.tx.try_send(msg) {
            warn!("Overlay channel send failed (dropping message): {}", e);
        }
    }
}

/// Start the single GTK thread and return a handle for the daemon.
///
/// The GTK `Application` is created exactly once here and runs for the process
/// lifetime.
pub fn start() -> OverlayHandle {
    let (tx, rx) = async_channel::bounded::<OverlayMsg>(CHANNEL_CAPACITY);

    std::thread::spawn(move || {
        let app = Application::builder().application_id(APP_ID).build();

        app.connect_activate(move |app| {
            build_overlay(app, rx.clone());
        });

        info!("Starting GTK application (single, long-lived)");
        // Run with no CLI args; hold the app active even with no visible window.
        let _hold = app.hold();
        app.run_with_args::<&str>(&[]);
    });

    OverlayHandle { tx }
}

/// Build the overlay window, wire the layer-shell surface, and attach the
/// message receiver to the glib main context.
fn build_overlay(app: &Application, rx: async_channel::Receiver<OverlayMsg>) {
    let window = ApplicationWindow::builder()
        .application(app)
        .default_width(crate::geometry::DEFAULT_OVERLAY_WIDTH)
        .default_height(OVERLAY_HEIGHT)
        .resizable(false)
        .decorated(false)
        .build();

    // --- layer-shell setup ---
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::None);
    window.set_namespace(Some(NAMESPACE));
    // Default anchor (bottom-left); corrected by the first Reposition.
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Bottom, true);

    apply_css(&window);

    // --- widget tree ---
    let hbox = GtkBox::new(Orientation::Horizontal, 6);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(4);
    hbox.set_margin_bottom(4);

    let menu_button = build_menu_button(app);

    let spaces_box = GtkBox::new(Orientation::Horizontal, 4);
    spaces_box.set_halign(gtk4::Align::Center);

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::External)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .hexpand(true)
        .propagate_natural_width(false)
        .kinetic_scrolling(true)
        .has_frame(false)
        .build();
    scrolled.set_overlay_scrolling(true);
    scrolled.set_child(Some(&spaces_box));

    // Scroll anywhere within the bar scrolls the space list.
    let scroll_controller =
        gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
    let scrolled_for_scroll = scrolled.clone();
    scroll_controller.connect_scroll(move |_, dx, dy| {
        let adj = scrolled_for_scroll.hadjustment();
        let current = adj.value();
        let step = 10.0;
        adj.set_value(current + (dy * step) + (dx * step));
        glib::Propagation::Stop
    });
    scrolled.add_controller(scroll_controller);

    let close_button = Button::with_label("✕");
    close_button.set_width_request(28);
    close_button.set_height_request(28);
    close_button.add_css_class("close-button");
    close_button.set_cursor_from_name(Some("pointer"));
    close_button.connect_clicked(|_| {
        info!("Close button clicked, requesting graceful daemon shutdown via IPC");
        ipc_helpers::shutdown_daemon();
    });

    hbox.append(&menu_button);
    hbox.append(&scrolled);
    hbox.append(&close_button);
    window.set_child(Some(&hbox));

    // Map the surface, then start hidden until the daemon sends Show.
    window.present();
    window.set_visible(false);

    // Track the current button count for auto-scroll.
    let total_spaces = Rc::new(RefCell::new(0usize));

    // --- message receiver on the glib main context (no polling) ---
    let window_rx = window.clone();
    let spaces_box_rx = spaces_box.clone();
    let scrolled_rx = scrolled.clone();
    let app_rx = app.clone();
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                OverlayMsg::UpdateSpaces { spaces, current } => {
                    rebuild_spaces(&spaces_box_rx, &spaces);
                    *total_spaces.borrow_mut() = spaces.len();
                    if spaces.len() > 1 {
                        dialog_utils::auto_scroll_to_item(
                            &scrolled_rx,
                            current,
                            spaces.len(),
                            32.0,
                        );
                    }
                }
                OverlayMsg::Reposition {
                    anchor,
                    margin_x,
                    margin_y,
                    width,
                    monitor,
                } => {
                    apply_reposition(&window_rx, anchor, margin_x, margin_y, width, &monitor);
                }
                OverlayMsg::Show => {
                    window_rx.set_visible(true);
                }
                OverlayMsg::Hide => {
                    window_rx.set_visible(false);
                }
                OverlayMsg::Shutdown => {
                    info!("Overlay received Shutdown, quitting GTK application");
                    window_rx.close();
                    app_rx.quit();
                    break;
                }
            }
        }
    });
}

fn build_menu_button(app: &Application) -> gtk4::MenuButton {
    let menu_button = gtk4::MenuButton::new();
    menu_button.set_icon_name("open-menu-symbolic");
    menu_button.set_width_request(28);
    menu_button.set_height_request(28);
    menu_button.add_css_class("menu-button");
    menu_button.set_cursor_from_name(Some("pointer"));

    let menu = gtk4::gio::Menu::new();
    menu.append(Some("New Space..."), Some("app.new_space"));
    menu.append(Some("Reset Position"), Some("app.reset_position"));
    menu.append(Some("Settings"), Some("app.settings"));

    let popover = gtk4::PopoverMenu::builder()
        .menu_model(&menu)
        .has_arrow(false)
        .build();
    menu_button.set_popover(Some(&popover));

    // Register actions once (guard against re-activation adding duplicates).
    if app.lookup_action("new_space").is_none() {
        let new_space_action = gtk4::gio::SimpleAction::new("new_space", None);
        new_space_action.connect_activate(move |_, _| {
            info!("New Space clicked, opening window");
            template_dialogs::show_new_space_window();
        });
        app.add_action(&new_space_action);

        let reset_position_action = gtk4::gio::SimpleAction::new("reset_position", None);
        reset_position_action.connect_activate(move |_, _| {
            info!("Reset Position clicked");
            ipc_helpers::reset_overlay_position();
        });
        app.add_action(&reset_position_action);

        let app_clone = app.clone();
        let settings_action = gtk4::gio::SimpleAction::new("settings", None);
        settings_action.connect_activate(move |_, _| {
            info!("Settings clicked, opening settings dialog");
            settings_dialog::show_settings_dialog(&app_clone);
        });
        app.add_action(&settings_action);
    }

    menu_button
}

fn rebuild_spaces(spaces_box: &GtkBox, spaces: &[SpaceButton]) {
    while let Some(child) = spaces_box.first_child() {
        spaces_box.remove(&child);
    }
    let total = spaces.len();
    for (index, sb) in spaces.iter().enumerate() {
        let button =
            ui_components::create_space_button(index, sb.label.clone(), sb.is_current, total);
        spaces_box.append(&button);
    }
}

fn apply_reposition(
    window: &ApplicationWindow,
    anchor: Anchor,
    margin_x: i32,
    margin_y: i32,
    width: i32,
    monitor: &str,
) {
    // Best-effort: pin the surface to the monitor holding the tracked window so
    // monitor-local margins land on the right output (multi-monitor).
    set_monitor_by_name(window, monitor);

    // Reset all anchors, then set the two for this corner.
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, false);
    }
    let (h_edge, v_edge) = match anchor {
        Anchor::BotLeft => (Edge::Left, Edge::Bottom),
        Anchor::BotRight => (Edge::Right, Edge::Bottom),
        Anchor::TopLeft => (Edge::Left, Edge::Top),
        Anchor::TopRight => (Edge::Right, Edge::Top),
    };
    window.set_anchor(h_edge, true);
    window.set_anchor(v_edge, true);
    window.set_margin(h_edge, margin_x);
    window.set_margin(v_edge, margin_y);

    let w = width.max(1);
    window.set_width_request(w);
    window.set_default_width(w);
    window.set_height_request(OVERLAY_HEIGHT);
}

fn set_monitor_by_name(window: &ApplicationWindow, monitor: &str) {
    if monitor.is_empty() {
        return;
    }
    let Some(display) = gtk4::gdk::Display::default() else {
        return;
    };
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        if let Some(obj) = monitors.item(i) {
            if let Ok(mon) = obj.downcast::<gtk4::gdk::Monitor>() {
                if mon.connector().as_deref() == Some(monitor) {
                    window.set_monitor(Some(&mon));
                    return;
                }
            }
        }
    }
}

fn apply_css(window: &ApplicationWindow) {
    let css = gtk4::CssProvider::new();
    css.load_from_data(OVERLAY_CSS);
    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

const OVERLAY_CSS: &str = r#"
window {
    background-color: rgba(30, 30, 30, 0.95);
    border-radius: 6px;
}
scrolledwindow { background: transparent; border: none; }
scrolledwindow > scrollbar,
scrolledwindow > scrollbar > slider {
    background: transparent;
    border: none;
    min-width: 0px;
    min-height: 0px;
    opacity: 0;
}
button {
    background: transparent;
    color: #cccccc;
    border-radius: 4px;
    border: none;
    font-size: 16px;
    min-width: 28px;
    min-height: 28px;
    padding: 0px;
    margin: 0px;
}
button:hover { background: rgba(255, 255, 255, 0.1); color: #ffffff; }
button:active { background: rgba(255, 255, 255, 0.15); }
button.space-button { color: #aaaaaa; font-size: 14px; }
button.space-button:hover { color: #ffffff; background: rgba(255, 255, 255, 0.15); }
button.space-button-current {
    color: #ffffff;
    font-size: 14px;
    font-weight: bold;
    background: rgba(100, 150, 255, 0.3);
    border: 1px solid rgba(100, 150, 255, 0.5);
}
button.space-button-current:hover { background: rgba(100, 150, 255, 0.4); }
button.close-button { font-size: 14px; color: #999999; }
button.close-button:hover { color: #ff6666; background: rgba(255, 102, 102, 0.1); }
button.menu-button { font-size: 18px; font-weight: bold; }
popover {
    background-color: rgba(30, 30, 30, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    padding: 0;
}
popover > contents { background-color: rgba(30, 30, 30, 0.95); padding: 0; }
popover modelbutton {
    background-color: transparent;
    color: #e0e0e0;
    border-radius: 0;
    padding: 8px 16px;
    min-width: 100px;
}
popover modelbutton:hover { background-color: rgba(255, 255, 255, 0.1); color: #ffffff; }
button.context-menu-item {
    background-color: transparent;
    color: #e0e0e0;
    border-radius: 0;
    padding: 8px 16px;
    min-width: 120px;
    font-size: 14px;
}
button.context-menu-item:hover { background-color: rgba(255, 255, 255, 0.1); color: #ffffff; }
"#;
