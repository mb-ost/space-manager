use gtk4::prelude::*;
use gtk4::{Button, Entry, Label, Orientation};
use tracing::info;

use super::ipc_helpers;

/// Create a space button with all event handlers (click and right-click context menu)
pub fn create_space_button(
    index: usize,
    space_num: String,
    is_current: bool,
    total_spaces: usize,
) -> Button {
    let space_button = Button::builder()
        .label(&space_num)
        .width_request(28)
        .height_request(28)
        .build();

    if is_current {
        space_button.add_css_class("space-button-current");
    } else {
        space_button.add_css_class("space-button");
    }

    // Connect left-click handler to switch to this space
    let target_index = index;
    let space_num_clone = space_num.clone();
    space_button.connect_clicked(move |_| {
        info!("Space button {} clicked, switching to space {}", space_num_clone, target_index);
        ipc_helpers::switch_to_space(target_index);
    });

    // Add right-click context menu
    attach_context_menu(&space_button, index, total_spaces);

    space_button.set_cursor_from_name(Some("pointer"));
    space_button
}

/// Attach a right-click context menu to a space button
fn attach_context_menu(space_button: &Button, context_index: usize, total_spaces: usize) {
    let gesture = gtk4::GestureClick::new();
    gesture.set_button(3); // Right mouse button

    let space_button_clone = space_button.clone();
    gesture.connect_pressed(move |gesture, _, _x, _y| {
        info!("Right-click on space button {}", context_index);

        // Create context menu popover
        let popover = gtk4::Popover::new();
        popover.set_has_arrow(false);
        popover.set_parent(&space_button_clone);

        let menu_box = gtk4::Box::new(Orientation::Vertical, 0);

        // Add menu items
        add_move_left_item(&menu_box, &popover, context_index, total_spaces);
        add_move_right_item(&menu_box, &popover, context_index, total_spaces);
        add_change_icon_item(&menu_box, &popover, context_index);
        add_close_space_item(&menu_box, &popover, context_index);

        popover.set_child(Some(&menu_box));
        popover.popup();

        gesture.set_state(gtk4::EventSequenceState::Claimed);
    });

    space_button.add_controller(gesture);
}

/// Add "Move Left" menu item (only if not the first space)
fn add_move_left_item(
    menu_box: &gtk4::Box,
    popover: &gtk4::Popover,
    context_index: usize,
    _total_spaces: usize,
) {
    if context_index == 0 {
        return; // Can't move left from first position
    }

    let move_left_btn = Button::with_label("← Move Left");
    move_left_btn.add_css_class("context-menu-item");
    move_left_btn.set_cursor_from_name(Some("pointer"));

    let popover_clone = popover.clone();
    move_left_btn.connect_clicked(move |_| {
        info!("Move left clicked for index {}", context_index);
        popover_clone.popdown();
        ipc_helpers::swap_windows(context_index, context_index - 1);
    });

    menu_box.append(&move_left_btn);
}

/// Add "Move Right" menu item (only if not the last space)
fn add_move_right_item(
    menu_box: &gtk4::Box,
    popover: &gtk4::Popover,
    context_index: usize,
    total_spaces: usize,
) {
    if context_index >= total_spaces - 1 {
        return; // Can't move right from last position
    }

    let move_right_btn = Button::with_label("Move Right →");
    move_right_btn.add_css_class("context-menu-item");
    move_right_btn.set_cursor_from_name(Some("pointer"));

    let popover_clone = popover.clone();
    move_right_btn.connect_clicked(move |_| {
        info!("Move right clicked for index {}", context_index);
        popover_clone.popdown();
        ipc_helpers::swap_windows(context_index, context_index + 1);
    });

    menu_box.append(&move_right_btn);
}

/// Add "Change Icon" menu item
fn add_change_icon_item(menu_box: &gtk4::Box, popover: &gtk4::Popover, context_index: usize) {
    let change_icon_btn = Button::with_label("✏ Change Icon");
    change_icon_btn.add_css_class("context-menu-item");
    change_icon_btn.set_cursor_from_name(Some("pointer"));

    let popover_clone = popover.clone();
    change_icon_btn.connect_clicked(move |_| {
        info!("Change icon clicked for index {}", context_index);
        popover_clone.popdown();
        show_change_icon_dialog(context_index);
    });

    menu_box.append(&change_icon_btn);
}

/// Add "Close Space" menu item
fn add_close_space_item(menu_box: &gtk4::Box, popover: &gtk4::Popover, context_index: usize) {
    let close_space_btn = Button::with_label("✕ Close Space");
    close_space_btn.add_css_class("context-menu-item");
    close_space_btn.add_css_class("destructive-action");
    close_space_btn.set_cursor_from_name(Some("pointer"));

    let popover_clone = popover.clone();
    close_space_btn.connect_clicked(move |_| {
        info!("Close space clicked for index {}", context_index);
        popover_clone.popdown();
        ipc_helpers::close_space(context_index);
    });

    menu_box.append(&close_space_btn);
}

/// Show dialog to change the icon for a space
fn show_change_icon_dialog(context_index: usize) {
    let dialog = gtk4::Window::builder()
        .title("Change Space Icon")
        .default_width(300)
        .default_height(150)
        .modal(true)
        .build();

    let vbox = gtk4::Box::new(Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);

    let label = Label::new(Some("Enter icon (emoji or text):"));
    let entry = Entry::new();
    entry.set_placeholder_text(Some("e.g. 🌐 or Web"));

    let button_box = gtk4::Box::new(Orientation::Horizontal, 12);
    button_box.set_halign(gtk4::Align::End);

    let cancel_btn = Button::with_label("Cancel");
    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        dialog_clone.close();
    });

    let ok_btn = Button::with_label("OK");
    ok_btn.add_css_class("suggested-action");
    let entry_clone = entry.clone();
    let dialog_clone2 = dialog.clone();
    ok_btn.connect_clicked(move |_| {
        let new_icon = entry_clone.text().to_string();
        dialog_clone2.close();
        ipc_helpers::set_window_icon(context_index, new_icon);
    });

    button_box.append(&cancel_btn);
    button_box.append(&ok_btn);

    vbox.append(&label);
    vbox.append(&entry);
    vbox.append(&button_box);

    dialog.set_child(Some(&vbox));
    dialog.present();
}

