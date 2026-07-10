//! Settings dialog (AF-7), moved out of the old `manager.rs`.
//!
//! Remains a regular GTK window. It writes `config.json` directly and then asks
//! the daemon to reload via `ipc_helpers::reload_config`. No `FollowMouseGuard`
//! is used here: under the layer-shell overlay a config reload no longer moves a
//! Hyprland client, so there is no focus to steal (OQ-2).

use gtk4::prelude::*;
use gtk4::{
    Application, Box as GtkBox, Button, CheckButton, ComboBoxText, Entry, Grid, Label, Orientation,
    ScrolledWindow, Window,
};
use tracing::{error, info};

use super::dialog_utils;
use super::ipc_helpers;
use super::theme;
use super::window_utils;

fn config_path() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".space-manager")
                .join("config.json")
        })
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.space-manager/config.json"))
}

/// Open the settings dialog attached to the given application.
pub fn show_settings_dialog(app: &Application) {
    info!("Creating settings dialog");

    let config_file = config_path();

    let settings = std::fs::read_to_string(&config_file)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());

    let dialog = Window::builder()
        .application(app)
        .title("Space Manager Settings")
        .default_width(500)
        .default_height(600)
        .modal(true)
        .build();

    window_utils::apply_float_center_with_size("Space Manager Settings", 500, 600);

    let main_box = dialog_utils::create_standard_container();
    let scrolled = ScrolledWindow::builder().vexpand(true).build();
    let grid = Grid::builder().row_spacing(12).column_spacing(12).build();

    let mut row = 0;

    let side_mouse_label = Label::new(Some("Enable Side Mouse Buttons:"));
    side_mouse_label.set_halign(gtk4::Align::Start);
    let side_mouse_check = CheckButton::new();
    side_mouse_check.set_active(
        settings
            .as_ref()
            .and_then(|s| s["side_mouse_binds"].as_bool())
            .unwrap_or(true),
    );
    grid.attach(&side_mouse_label, 0, row, 1, 1);
    grid.attach(&side_mouse_check, 1, row, 1, 1);
    row += 1;

    let overlay_enabled_label = Label::new(Some("Enable Overlay:"));
    overlay_enabled_label.set_halign(gtk4::Align::Start);
    let overlay_enabled_check = CheckButton::new();
    overlay_enabled_check.set_active(
        settings
            .as_ref()
            .and_then(|s| s["overlay"]["enabled"].as_bool())
            .unwrap_or(true),
    );
    grid.attach(&overlay_enabled_label, 0, row, 1, 1);
    grid.attach(&overlay_enabled_check, 1, row, 1, 1);
    row += 1;

    let from_area_label = Label::new(Some("Mouse Change Area Position:"));
    from_area_label.set_halign(gtk4::Align::Start);
    let from_area_combo = ComboBoxText::new();
    from_area_combo.append(Some("left"), "Left");
    from_area_combo.append(Some("right"), "Right");
    from_area_combo.append(Some("top"), "Top");
    from_area_combo.append(Some("bottom"), "Bottom");
    let from_area_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["from_area"].as_str())
        .unwrap_or("left");
    from_area_combo.set_active_id(Some(from_area_value));
    grid.attach(&from_area_label, 0, row, 1, 1);
    grid.attach(&from_area_combo, 1, row, 1, 1);
    row += 1;

    let from_overlay_label = Label::new(Some("Overlay Position:"));
    from_overlay_label.set_halign(gtk4::Align::Start);
    let from_overlay_combo = ComboBoxText::new();
    from_overlay_combo.append(Some("bot_left"), "Bottom Left");
    from_overlay_combo.append(Some("bot_right"), "Bottom Right");
    from_overlay_combo.append(Some("top_left"), "Top Left");
    from_overlay_combo.append(Some("top_right"), "Top Right");
    let from_overlay_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["from_overlay"].as_str())
        .unwrap_or("bot_left");
    from_overlay_combo.set_active_id(Some(from_overlay_value));
    grid.attach(&from_overlay_label, 0, row, 1, 1);
    grid.attach(&from_overlay_combo, 1, row, 1, 1);
    row += 1;

    let overlay_size_label = Label::new(Some("Overlay Width:"));
    overlay_size_label.set_halign(gtk4::Align::Start);
    let overlay_size_entry = Entry::new();
    let overlay_size_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["overlay_size"].as_str())
        .unwrap_or("change_area_x");
    overlay_size_entry.set_text(overlay_size_value);
    overlay_size_entry.set_tooltip_text(Some(
        "change_area_x, change_area_y, or pixel value (e.g. 250)",
    ));
    grid.attach(&overlay_size_label, 0, row, 1, 1);
    grid.attach(&overlay_size_entry, 1, row, 1, 1);
    row += 1;

    let offset_x_label = Label::new(Some("Horizontal Offset (px):"));
    offset_x_label.set_halign(gtk4::Align::Start);
    let offset_x_entry = Entry::new();
    let offset_x_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["offset_x"].as_i64())
        .unwrap_or(8);
    offset_x_entry.set_text(&offset_x_value.to_string());
    grid.attach(&offset_x_label, 0, row, 1, 1);
    grid.attach(&offset_x_entry, 1, row, 1, 1);
    row += 1;

    let offset_y_label = Label::new(Some("Vertical Offset (px):"));
    offset_y_label.set_halign(gtk4::Align::Start);
    let offset_y_entry = Entry::new();
    let offset_y_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["offset_y"].as_i64())
        .unwrap_or(26);
    offset_y_entry.set_text(&offset_y_value.to_string());
    grid.attach(&offset_y_label, 0, row, 1, 1);
    grid.attach(&offset_y_entry, 1, row, 1, 1);
    row += 1;

    let fraction_label = Label::new(Some("Change Area Fraction:"));
    fraction_label.set_halign(gtk4::Align::Start);
    let fraction_entry = Entry::new();
    let fraction_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["change_area_fraction"].as_f64())
        .unwrap_or(0.125);
    fraction_entry.set_text(&fraction_value.to_string());
    fraction_entry.set_tooltip_text(Some("Fraction of window dimension (e.g., 0.125 = 1/8)"));
    grid.attach(&fraction_label, 0, row, 1, 1);
    grid.attach(&fraction_entry, 1, row, 1, 1);
    row += 1;

    let min_px_label = Label::new(Some("Min Change Area (px):"));
    min_px_label.set_halign(gtk4::Align::Start);
    let min_px_entry = Entry::new();
    let min_px_value = settings
        .as_ref()
        .and_then(|s| s["overlay"]["min_change_area_px"].as_i64())
        .unwrap_or(250);
    min_px_entry.set_text(&min_px_value.to_string());
    grid.attach(&min_px_label, 0, row, 1, 1);
    grid.attach(&min_px_entry, 1, row, 1, 1);

    scrolled.set_child(Some(&grid));
    main_box.append(&scrolled);

    let button_box = GtkBox::new(Orientation::Horizontal, 12);
    button_box.set_halign(gtk4::Align::End);

    let cancel_button = Button::with_label("Cancel");
    let apply_button = Button::with_label("Apply");
    let save_button = Button::with_label("Save");
    save_button.add_css_class("suggested-action");

    let dialog_clone = dialog.clone();
    cancel_button.connect_clicked(move |_| {
        dialog_clone.close();
    });

    // Shared closure to persist current form values to config.json.
    let widgets = FormWidgets {
        side_mouse_check: side_mouse_check.clone(),
        overlay_enabled_check: overlay_enabled_check.clone(),
        from_area_combo: from_area_combo.clone(),
        from_overlay_combo: from_overlay_combo.clone(),
        overlay_size_entry: overlay_size_entry.clone(),
        offset_x_entry: offset_x_entry.clone(),
        offset_y_entry: offset_y_entry.clone(),
        fraction_entry: fraction_entry.clone(),
        min_px_entry: min_px_entry.clone(),
    };

    let config_file_apply = config_file.clone();
    let widgets_apply = widgets.clone();
    apply_button.connect_clicked(move |_| {
        info!("Applying settings...");
        if save_settings(&config_file_apply, &widgets_apply) {
            ipc_helpers::reload_config();
        }
    });

    let config_file_save = config_file.clone();
    let widgets_save = widgets.clone();
    let dialog_clone2 = dialog.clone();
    save_button.connect_clicked(move |_| {
        info!("Saving settings...");
        if save_settings(&config_file_save, &widgets_save) {
            ipc_helpers::reload_config();
        }
        dialog_clone2.close();
    });

    button_box.append(&cancel_button);
    button_box.append(&apply_button);
    button_box.append(&save_button);
    main_box.append(&button_box);

    dialog.set_child(Some(&main_box));
    dialog.add_css_class("settings-dialog");
    theme::apply_template_window_theme(&dialog);

    dialog.present();
}

#[derive(Clone)]
struct FormWidgets {
    side_mouse_check: CheckButton,
    overlay_enabled_check: CheckButton,
    from_area_combo: ComboBoxText,
    from_overlay_combo: ComboBoxText,
    overlay_size_entry: Entry,
    offset_x_entry: Entry,
    offset_y_entry: Entry,
    fraction_entry: Entry,
    min_px_entry: Entry,
}

/// Persist the form to `config.json`, preserving unrelated fields (templates).
/// Returns true on success.
fn save_settings(config_file: &std::path::Path, w: &FormWidgets) -> bool {
    let mut existing_config = std::fs::read_to_string(config_file)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    existing_config["side_mouse_binds"] = serde_json::json!(w.side_mouse_check.is_active());
    existing_config["overlay"] = serde_json::json!({
        "enabled": w.overlay_enabled_check.is_active(),
        "from_area": w.from_area_combo.active_id().map(|s| s.to_string()).unwrap_or_else(|| "left".to_string()),
        "from_overlay": w.from_overlay_combo.active_id().map(|s| s.to_string()).unwrap_or_else(|| "bot_left".to_string()),
        "overlay_size": w.overlay_size_entry.text().to_string(),
        "offset_x": w.offset_x_entry.text().parse::<i32>().unwrap_or(8),
        "offset_y": w.offset_y_entry.text().parse::<i32>().unwrap_or(26),
        "change_area_fraction": w.fraction_entry.text().parse::<f64>().unwrap_or(0.125),
        "min_change_area_px": w.min_px_entry.text().parse::<i32>().unwrap_or(250),
    });
    existing_config["mouse"] = serde_json::json!({
        "change_area_fraction": w.fraction_entry.text().parse::<f64>().unwrap_or(0.125),
        "min_change_area_px": w.min_px_entry.text().parse::<i32>().unwrap_or(250),
    });

    match serde_json::to_string_pretty(&existing_config) {
        Ok(content) => {
            if let Err(e) = std::fs::write(config_file, content) {
                error!("Failed to save settings: {}", e);
                false
            } else {
                info!("Settings saved successfully");
                true
            }
        }
        Err(e) => {
            error!("Failed to serialize settings: {}", e);
            false
        }
    }
}
