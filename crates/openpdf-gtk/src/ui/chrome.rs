// SPDX-License-Identifier: MPL-2.0
use gtk::glib::{self, clone};
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::i18n::t;
use crate::Lang;

pub struct ToolRow {
    pub root: gtk::Box,
    pub zoom_label: gtk::Label,
    pub search_entry: gtk::SearchEntry,
    pub settings_btn: gtk::MenuButton,
}

pub fn measuring() -> bool {
    gtk::gio::Application::default()
        .and_then(|app| app.application_id())
        .is_some_and(|id| id == crate::MEASURE_APP_ID)
}

pub fn tab_row(tab_bar: &adw::TabBar, on_new: impl Fn() + 'static) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("tab-row");
    row.set_hexpand(true);

    let icon = gtk::Image::new();
    icon.set_pixel_size(16);
    icon.set_margin_start(8);
    row.append(&icon);
    if !measuring() {
        glib::idle_add_local_once(clone!(@strong icon => move || {
            icon.set_from_icon_name(Some(crate::APP_ID));
        }));
    }

    tab_bar.set_hexpand(true);
    tab_bar.set_autohide(false);
    row.append(tab_bar);

    let add = gtk::Button::with_label("+");
    add.add_css_class("flat");
    add.add_css_class("circular");
    add.set_tooltip_text(Some(t(Lang::En, "Open…")));
    add.connect_clicked(move |_| on_new());
    row.append(&add);

    row.append(&gtk::WindowControls::new(gtk::PackType::End));

    let handle = gtk::WindowHandle::new();
    handle.set_child(Some(&row));
    handle.upcast()
}

pub fn tool_row(lang: Lang, menu: &gtk::gio::Menu) -> ToolRow {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("tool-row");
    row.set_hexpand(true);

    let left = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let open = icon_action_button("document-open-symbolic", Some("app.open"), t(lang, "Open…"));
    let save = icon_action_button("document-save-symbolic", None, t(lang, "Save"));
    save.set_sensitive(false);
    let print = icon_action_button("document-print-symbolic", None, t(lang, "Print"));
    print.set_sensitive(false);
    left.append(&open);
    left.append(&save);
    left.append(&print);
    row.append(&left);

    let modes = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    modes.add_css_class("mode-bar");
    let view = gtk::ToggleButton::with_label(t(lang, "View"));
    view.set_active(true);
    view.add_css_class("mode-btn");
    view.add_css_class("pill");
    view.add_css_class("suggested-action");
    modes.append(&view);
    for (label, action) in [
        (t(lang, "Comment"), None),
        (t(lang, "Fill and sign"), Some("app.sign")),
        (t(lang, "Organize"), None),
        (t(lang, "Edit"), None),
        (t(lang, "Protect"), None),
        (t(lang, "Tools"), None),
    ] {
        let btn = gtk::Button::with_label(label);
        btn.add_css_class("mode-btn");
        btn.add_css_class("flat");
        btn.add_css_class("pill");
        if let Some(name) = action {
            btn.set_action_name(Some(name));
        } else {
            btn.set_sensitive(false);
            btn.set_tooltip_text(Some(t(lang, "Waiting on openpdf-core")));
        }
        modes.append(&btn);
    }
    row.append(&modes);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(t(lang, "Find in document Ctrl+F")));
    search.set_width_chars(22);
    row.append(&search);

    let zoom_out = gtk::Button::with_label("\u{2212}");
    zoom_out.add_css_class("flat");
    zoom_out.add_css_class("mode-btn");
    zoom_out.set_action_name(Some("win.zoom-out"));
    let zoom_label = gtk::Label::new(Some("125 %"));
    zoom_label.add_css_class("dim-label");
    let zoom_in = gtk::Button::with_label("+");
    zoom_in.add_css_class("flat");
    zoom_in.add_css_class("mode-btn");
    zoom_in.set_action_name(Some("win.zoom-in"));
    row.append(&zoom_out);
    row.append(&zoom_label);
    row.append(&zoom_in);

    let settings_btn = gtk::MenuButton::new();
    settings_btn.add_css_class("flat");
    settings_btn.set_menu_model(Some(menu));
    settings_btn.set_tooltip_text(Some(t(lang, "Preferences")));
    row.append(&settings_btn);

    ToolRow {
        root: row,
        zoom_label,
        search_entry: search,
        settings_btn,
    }
}

fn icon_action_button(icon: &'static str, action: Option<&str>, tooltip: &str) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.add_css_class("flat");
    btn.set_tooltip_text(Some(tooltip));
    if let Some(action) = action {
        btn.set_action_name(Some(action));
    }
    glib::idle_add_local_once(clone!(@strong btn => move || {
        btn.set_icon_name(icon);
    }));
    btn
}
