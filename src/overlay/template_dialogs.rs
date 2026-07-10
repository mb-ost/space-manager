//! New-space / template dialogs (AF-7), moved out of the old `manager.rs`.
//!
//! These remain regular GTK windows (not layer-shell). They talk to the daemon
//! exclusively through `ipc_helpers`.

use gtk4::prelude::*;
use tracing::info;

use super::dialog_utils;
use super::ipc_helpers;
use super::theme;
use super::window_utils;

/// Open the "New Space" dialog (template list is the initial view).
pub fn show_new_space_window() {
    info!("Creating new space window");

    let dialog = gtk4::Window::builder()
        .title("New Space")
        .default_width(500)
        .default_height(400)
        .modal(true)
        .build();

    window_utils::apply_float_center_with_size("New Space", 500, 400);
    theme::apply_template_window_theme(&dialog);

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    dialog.set_child(Some(&container));

    show_template_list_view(&dialog, &container);

    dialog.present();
}

fn show_template_list_view(dialog: &gtk4::Window, container: &gtk4::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);

    let title_label = gtk4::Label::new(Some("Create New Space"));
    title_label.add_css_class("title-label");
    vbox.append(&title_label);

    let templates = std::thread::spawn(ipc_helpers::get_templates_sync)
        .join()
        .ok()
        .flatten();

    let scrolled = gtk4::ScrolledWindow::builder().vexpand(true).build();
    let templates_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);

    if let Some(templates_arr) = templates.and_then(|t| t.as_array().cloned()) {
        if templates_arr.is_empty() {
            let empty_label = gtk4::Label::new(Some("No templates yet. Create one below!"));
            empty_label.add_css_class("dim-label");
            templates_box.append(&empty_label);
        } else {
            for template in templates_arr {
                let name = template["name"].as_str().unwrap_or("Unknown").to_string();
                let command = template["command"].as_str().unwrap_or("").to_string();

                let item_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                item_box.set_margin_start(8);
                item_box.set_margin_end(8);
                item_box.set_margin_top(4);
                item_box.set_margin_bottom(4);

                let template_btn = gtk4::Button::with_label(&name);
                template_btn.set_hexpand(true);
                template_btn.add_css_class("template-button");
                template_btn.set_cursor_from_name(Some("pointer"));

                let container_clone = container.clone();
                let dialog_clone = dialog.clone();
                let command_clone = command.clone();
                let name_clone = name.clone();
                template_btn.connect_clicked(move |_| {
                    info!("Template selected: {}", name_clone);
                    show_template_use_view(&dialog_clone, &container_clone, &command_clone);
                });

                let delete_btn = gtk4::Button::with_label("🗑");
                delete_btn.set_width_request(36);
                delete_btn.add_css_class("delete-button");
                delete_btn.set_cursor_from_name(Some("pointer"));
                delete_btn.set_tooltip_text(Some("Delete this template"));

                let name_for_delete = name.clone();
                let container_clone2 = container.clone();
                let dialog_clone2 = dialog.clone();
                delete_btn.connect_clicked(move |_| {
                    info!("Delete template: {}", name_for_delete);
                    ipc_helpers::remove_template(name_for_delete.clone());
                    show_template_list_view(&dialog_clone2, &container_clone2);
                });

                item_box.append(&template_btn);
                item_box.append(&delete_btn);
                templates_box.append(&item_box);
            }
        }
    }

    scrolled.set_child(Some(&templates_box));
    vbox.append(&scrolled);

    let button_box = dialog_utils::create_button_box();

    let add_template_btn = gtk4::Button::with_label("✚ Add Template");
    add_template_btn.set_cursor_from_name(Some("pointer"));

    let container_clone = container.clone();
    let dialog_clone = dialog.clone();
    add_template_btn.connect_clicked(move |_| {
        info!("Add Template clicked - switching to add template view");
        show_add_template_view(&dialog_clone, &container_clone);
    });

    let close_btn = gtk4::Button::with_label("Close");
    let dialog_clone2 = dialog.clone();
    close_btn.connect_clicked(move |_| {
        dialog_clone2.close();
    });

    button_box.append(&add_template_btn);
    button_box.append(&close_btn);
    vbox.append(&button_box);

    container.append(&vbox);
}

fn show_template_use_view(dialog: &gtk4::Window, container: &gtk4::Box, command_template: &str) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let re = regex::Regex::new(r"\{\{([^}]+)\}\}").unwrap();
    let mut variables: Vec<String> = vec![];
    for cap in re.captures_iter(command_template) {
        if let Some(var) = cap.get(1) {
            let var_name = var.as_str().to_string();
            if !variables.contains(&var_name) {
                variables.push(var_name);
            }
        }
    }

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);

    let title_label = gtk4::Label::new(Some("Create Space from Template"));
    title_label.add_css_class("title-label");
    vbox.append(&title_label);

    let template_label = gtk4::Label::new(Some("Template:"));
    template_label.set_halign(gtk4::Align::Start);
    template_label.add_css_class("field-label");

    let template_display = gtk4::Label::new(Some(command_template));
    template_display.set_halign(gtk4::Align::Start);
    template_display.set_wrap(true);
    template_display.add_css_class("template-display");

    vbox.append(&template_label);
    vbox.append(&template_display);

    let separator = gtk4::Separator::new(gtk4::Orientation::Horizontal);
    separator.set_margin_top(8);
    separator.set_margin_bottom(8);
    vbox.append(&separator);

    let icon_position_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);

    let icon_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let icon_label = gtk4::Label::new(Some("Icon:"));
    icon_label.set_halign(gtk4::Align::Start);
    icon_label.add_css_class("field-label");
    let icon_entry = gtk4::Entry::new();
    icon_entry.set_placeholder_text(Some("🌐"));
    icon_entry.set_hexpand(true);
    icon_vbox.append(&icon_label);
    icon_vbox.append(&icon_entry);

    let position_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let position_label = gtk4::Label::new(Some("Position:"));
    position_label.set_halign(gtk4::Align::Start);
    position_label.add_css_class("field-label");
    let position_entry = gtk4::Entry::new();
    position_entry.set_placeholder_text(Some("1, 2, 3..."));
    position_entry.set_width_chars(10);
    position_vbox.append(&position_label);
    position_vbox.append(&position_entry);

    icon_position_box.append(&icon_vbox);
    icon_position_box.append(&position_vbox);
    vbox.append(&icon_position_box);

    let mut variable_entries: Vec<(String, gtk4::Entry)> = vec![];
    for var in &variables {
        let var_label = gtk4::Label::new(Some(&format!("{}:", var)));
        var_label.set_halign(gtk4::Align::Start);
        var_label.add_css_class("field-label");
        let var_entry = gtk4::Entry::new();
        var_entry.set_placeholder_text(Some(&format!("Value for {{{{{}}}}}", var)));

        vbox.append(&var_label);
        vbox.append(&var_entry);
        variable_entries.push((var.clone(), var_entry));
    }

    let button_box = dialog_utils::create_button_box();

    let cancel_btn = dialog_utils::create_cancel_button();
    let container_clone = container.clone();
    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        info!("Cancel clicked - returning to template list");
        show_template_list_view(&dialog_clone, &container_clone);
    });

    let create_btn = dialog_utils::create_action_button("Create Space");
    let command_template_owned = command_template.to_string();
    let position_entry_clone = position_entry.clone();
    let icon_entry_clone = icon_entry.clone();
    let dialog_clone2 = dialog.clone();
    create_btn.connect_clicked(move |_| {
        let position_str = position_entry_clone.text().to_string();
        let position_opt: Option<usize> = if position_str.is_empty() {
            None
        } else {
            position_str
                .parse::<usize>()
                .ok()
                .and_then(|p| if p > 0 { Some(p - 1) } else { None })
        };

        let icon = icon_entry_clone.text().to_string();
        let icon_opt = if icon.is_empty() { None } else { Some(icon) };

        let mut final_command = command_template_owned.clone();
        for (var, entry) in &variable_entries {
            let value = entry.text().to_string();
            final_command = final_command.replace(&format!("{{{{{}}}}}", var), &value);
        }

        info!("Spawning with command: {}", final_command);
        dialog_clone2.close();

        if let Some(idx) = position_opt {
            ipc_helpers::spawn_at(idx, final_command, icon_opt);
        } else {
            let cmd = serde_json::json!({ "Spawn": final_command });
            ipc_helpers::send_command_async(cmd);
        }
    });

    button_box.append(&cancel_btn);
    button_box.append(&create_btn);
    vbox.append(&button_box);

    container.append(&vbox);
}

fn show_add_template_view(dialog: &gtk4::Window, container: &gtk4::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);
    vbox.set_margin_top(20);
    vbox.set_margin_bottom(20);
    let title_label = gtk4::Label::new(Some("Add Command Template"));
    title_label.add_css_class("title-label");
    vbox.append(&title_label);

    let name_label = gtk4::Label::new(Some("Template Name:"));
    name_label.set_halign(gtk4::Align::Start);
    name_label.add_css_class("field-label");
    let name_entry = gtk4::Entry::new();
    name_entry.set_placeholder_text(Some("e.g. Browser Profile"));
    vbox.append(&name_label);
    vbox.append(&name_entry);

    let command_label = gtk4::Label::new(Some("Command (use {{variable}} for placeholders):"));
    command_label.set_halign(gtk4::Align::Start);
    command_label.add_css_class("field-label");
    let command_entry = gtk4::Entry::new();
    command_entry.set_placeholder_text(Some(
        "e.g. brave --user-data-dir=\"$HOME/.config/{{profile}}\"",
    ));
    vbox.append(&command_label);
    vbox.append(&command_entry);

    let button_box = dialog_utils::create_button_box();
    let cancel_btn = dialog_utils::create_cancel_button();
    let container_clone = container.clone();
    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        info!("Cancel clicked - returning to template list");
        show_template_list_view(&dialog_clone, &container_clone);
    });

    let save_btn = dialog_utils::create_action_button("Save");
    let name_entry_clone = name_entry.clone();
    let command_entry_clone = command_entry.clone();
    let container_clone2 = container.clone();
    let dialog_clone2 = dialog.clone();
    save_btn.connect_clicked(move |_| {
        let name = name_entry_clone.text().to_string();
        let command = command_entry_clone.text().to_string();
        if name.is_empty() || command.is_empty() {
            return;
        }
        ipc_helpers::add_template(name, command);
        show_template_list_view(&dialog_clone2, &container_clone2);
    });
    button_box.append(&cancel_btn);
    button_box.append(&save_btn);
    vbox.append(&button_box);

    container.append(&vbox);
}
