/// Dialog builder utilities for consistent dialog creation
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Orientation, Window};

/// Builder for creating consistent dialog windows
pub struct DialogBuilder {
    title: String,
    width: i32,
    height: i32,
    modal: bool,
}

impl DialogBuilder {
    /// Create a new dialog builder with the given title
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            width: 400,
            height: 300,
            modal: true,
        }
    }

    /// Set the dialog width
    pub fn width(mut self, width: i32) -> Self {
        self.width = width;
        self
    }

    /// Set the dialog height
    pub fn height(mut self, height: i32) -> Self {
        self.height = height;
        self
    }

    /// Set whether the dialog is modal
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// Build the dialog window
    pub fn build(self) -> Window {
        Window::builder()
            .title(&self.title)
            .default_width(self.width)
            .default_height(self.height)
            .modal(self.modal)
            .build()
    }
}

/// Create a standard content container with consistent margins
pub fn create_standard_container() -> GtkBox {
    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    vbox
}

/// Create a standard button box for dialog buttons (aligned to the right)
pub fn create_button_box() -> GtkBox {
    let button_box = GtkBox::new(Orientation::Horizontal, 12);
    button_box.set_halign(Align::End);
    button_box
}

/// Create a standard cancel button
pub fn create_cancel_button() -> Button {
    let btn = Button::with_label("Cancel");
    btn.add_css_class("dialog-button");
    btn
}

/// Create a standard OK/Save button with suggested action styling
pub fn create_action_button(label: &str) -> Button {
    let btn = Button::with_label(label);
    btn.add_css_class("suggested-action");
    btn.add_css_class("dialog-button");
    btn
}

/// Auto-scroll a scrolled window to show the current item with context
///
/// # Arguments
/// * `scrolled_window` - The scrolled window to scroll
/// * `current_index` - Index of the current item
/// * `total_items` - Total number of items
/// * `item_width` - Approximate width of each item in pixels
pub fn auto_scroll_to_item(
    scrolled_window: &gtk4::ScrolledWindow,
    current_index: usize,
    total_items: usize,
    item_width: f64,
) {
    let scrolled_window = scrolled_window.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        let adj = scrolled_window.hadjustment();
        let viewport_width = adj.page_size();

        // Calculate position to show current item with 1 item context before if possible
        let target_pos = if total_items <= 1 {
            0.0
        } else {
            // Try to show 1 item before current if possible
            let ideal_start_index = if current_index > 0 {
                current_index - 1
            } else {
                0
            };

            let ideal_start = ideal_start_index as f64 * item_width;

            // Clamp to valid range
            let max_scroll = (total_items as f64 * item_width - viewport_width).max(0.0);
            ideal_start.min(max_scroll).max(0.0)
        };

        adj.set_value(target_pos);
    });
}
