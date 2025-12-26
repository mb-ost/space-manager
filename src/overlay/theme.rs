use gtk4::prelude::*;
use gtk4::CssProvider;

/// Apply consistent theme CSS to a window
/// This centralizes all CSS styling to avoid duplication
pub fn apply_template_window_theme(window: &gtk4::Window) {
    window.add_css_class("template-window");

    let css_provider = CssProvider::new();
    css_provider.load_from_data(TEMPLATE_WINDOW_CSS);

    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Apply overlay theme CSS
pub fn apply_overlay_theme(window: &gtk4::ApplicationWindow) {
    let css_provider = CssProvider::new();
    css_provider.load_from_data(OVERLAY_CSS);

    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &css_provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Centralized CSS for template windows (New Space, Settings, etc.)
const TEMPLATE_WINDOW_CSS: &str = r#"
window.template-window {
    background-color: #2b2b2b;
    color: #ffffff;
}

/* All labels should be white by default */
window.template-window label {
    color: #ffffff;
}

window.template-window label.title-label {
    color: #ffffff;
    font-size: 16px;
    font-weight: bold;
}

window.template-window label.field-label {
    color: #ffffff;
    font-size: 13px;
    font-weight: 500;
}

window.template-window label.dim-label {
    color: #888888;
    font-style: italic;
}

window.template-window label.template-display {
    color: #a0a0a0;
    font-size: 12px;
    font-style: italic;
    padding: 8px;
    background-color: #1e1e1e;
    border-radius: 4px;
}

/* Text entries */
window.template-window entry {
    background-color: #3c3c3c;
    color: #ffffff;
    border: 1px solid #555555;
    border-radius: 4px;
    padding: 6px;
    caret-color: #ffffff;
}

window.template-window entry text {
    color: #ffffff;
}

window.template-window entry:focus {
    border-color: #4a90e2;
}

window.template-window entry::placeholder {
    color: #888888;
}

window.template-window entry selection {
    background-color: #4a90e2;
    color: #ffffff;
}

/* Combobox styling */
window.template-window combobox {
    color: #ffffff;
}

window.template-window combobox button {
    background-color: #3c3c3c;
    color: #ffffff;
    border: 1px solid #555555;
    border-radius: 4px;
    padding: 6px;
}

window.template-window combobox button:hover {
    background-color: #4a4a4a;
}

window.template-window combobox button label {
    color: #ffffff;
}

/* Dropdown menu */
window.template-window popover {
    background-color: #2b2b2b;
    color: #ffffff;
}

window.template-window popover modelbutton {
    color: #ffffff;
}

window.template-window popover modelbutton:hover {
    background-color: #4a4a4a;
}

/* Checkbuttons */
window.template-window checkbutton {
    color: #ffffff;
}

window.template-window checkbutton label {
    color: #ffffff;
}

window.template-window checkbutton check {
    color: #ffffff;
    border-color: #555555;
}

/* Grid labels */
window.template-window grid label {
    color: #ffffff;
}

/* Box labels */
window.template-window box label {
    color: #ffffff;
}

/* Scrolled window content */
window.template-window scrolledwindow {
    color: #ffffff;
}

window.template-window scrolledwindow label {
    color: #ffffff;
}

/* Buttons */
window.template-window button.dialog-button,
window.template-window button.template-button {
    background: #4a4a4a;
    color: #ffffff;
    border-radius: 4px;
    border: 1px solid #555555;
    padding: 8px 16px;
}

window.template-window button.dialog-button label,
window.template-window button.template-button label {
    color: #ffffff;
}

window.template-window button.dialog-button:hover,
window.template-window button.template-button:hover {
    background: #5a5a5a;
}

window.template-window button.suggested-action {
    background: #4a90e2;
    color: #ffffff;
    border: 1px solid #357abd;
}

window.template-window button.suggested-action label {
    color: #ffffff;
}

window.template-window button.suggested-action:hover {
    background: #5aa0f2;
}

window.template-window button.delete-button {
    background: #d32f2f;
    color: #ffffff;
    border: 1px solid #b71c1c;
}

window.template-window button.delete-button label {
    color: #ffffff;
}

window.template-window button.delete-button:hover {
    background: #e53935;
}

window.template-window separator {
    background-color: #555555;
}

/* Settings dialog specific styling */
window.settings-dialog {
    background-color: #2b2b2b;
    color: #ffffff;
}

window.settings-dialog label {
    color: #ffffff;
    font-size: 13px;
}

window.settings-dialog grid label {
    color: #ffffff;
}

window.settings-dialog box label {
    color: #ffffff;
}

window.settings-dialog scrolledwindow label {
    color: #ffffff;
}

window.settings-dialog entry {
    background-color: #3c3c3c;
    color: #ffffff;
    border: 1px solid #555555;
    border-radius: 4px;
    padding: 6px;
    min-width: 200px;
    caret-color: #ffffff;
}

window.settings-dialog entry text {
    color: #ffffff;
}

window.settings-dialog entry:focus {
    border-color: #4a90e2;
}

window.settings-dialog entry::placeholder {
    color: #888888;
}

window.settings-dialog entry selection {
    background-color: #4a90e2;
    color: #ffffff;
}

window.settings-dialog button {
    background: #4a4a4a;
    color: #ffffff;
    border-radius: 4px;
    border: 1px solid #555555;
    padding: 8px 16px;
    font-size: 13px;
}

window.settings-dialog button label {
    color: #ffffff;
}

window.settings-dialog button:hover {
    background: #5a5a5a;
}

window.settings-dialog button.suggested-action {
    background: #4a90e2;
    color: #ffffff;
    border: 1px solid #357abd;
}

window.settings-dialog button.suggested-action label {
    color: #ffffff;
}

window.settings-dialog button.suggested-action:hover {
    background: #5aa0f2;
}

window.settings-dialog combobox {
    color: #ffffff;
}

window.settings-dialog combobox button {
    min-width: 200px;
    background-color: #3c3c3c;
    color: #ffffff;
    border: 1px solid #555555;
}

window.settings-dialog combobox button label {
    color: #ffffff;
}

window.settings-dialog combobox button:hover {
    background-color: #4a4a4a;
}

window.settings-dialog popover {
    background-color: #2b2b2b;
    color: #ffffff;
}

window.settings-dialog popover modelbutton {
    color: #ffffff;
}

window.settings-dialog popover modelbutton:hover {
    background-color: #4a4a4a;
}

window.settings-dialog checkbutton {
    color: #ffffff;
}

window.settings-dialog checkbutton label {
    color: #ffffff;
}

window.settings-dialog checkbutton check {
    color: #ffffff;
    border-color: #555555;
}
"#;

/// CSS for the main overlay window
const OVERLAY_CSS: &str = r#"
window {
    background: rgba(40, 40, 40, 0.95);
    border-radius: 8px;
}

.space-button {
    background: transparent;
    color: #aaaaaa;
    border: none;
    padding: 8px;
    font-size: 16px;
    min-width: 32px;
}

.space-button:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #ffffff;
}

.space-button.current {
    color: #4a90e2;
    font-weight: bold;
}

.close-button {
    background: transparent;
    color: #999999;
    border: none;
    padding: 4px;
    min-width: 20px;
    min-height: 20px;
    font-size: 14px;
}

.close-button:hover {
    background: rgba(255, 255, 255, 0.15);
    color: #ffffff;
}

.menu-button {
    background: transparent;
    color: #999999;
    border: none;
    padding: 4px;
    min-width: 20px;
    min-height: 20px;
    font-size: 16px;
}

.menu-button:hover {
    background: rgba(255, 255, 255, 0.15);
    color: #ffffff;
}
"#;

