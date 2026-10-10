// SPDX-License-Identifier: MPL-2.0
//! Per-signature verification dialog. Row text is core `format_verify_result`.
//!
//! Bytes come from [`crate::Opened::bytes_for_verify`], never `to_bytes()` —
//! a signed document's full rewrite is `Signed(Rewrite)`.

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{format_verify_result, format_verify_results, SignatureInfo};

use crate::i18n::t;
use crate::sign::{verify_css_class, verify_icon, verify_severity};
use crate::Lang;

/// Present every signature as its own row. An empty list still opens so a
/// manual Verify action can say “No signatures found” instead of failing
/// silently.
pub fn present(parent: &impl IsA<gtk::Window>, lang: Lang, infos: &[SignatureInfo]) {
    let win = adw::Window::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_default_size(480, 420);
    win.set_title(Some(t(lang, "Signatures")));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    if infos.is_empty() {
        let status = adw::StatusPage::new();
        status.set_icon_name(Some("dialog-password-symbolic"));
        status.set_title(t(lang, "Signatures"));
        status.set_description(Some(&format_verify_results(&[])));
        toolbar.set_content(Some(&status));
    } else {
        toolbar.set_content(Some(&list_body(infos)));
    }

    win.set_content(Some(&toolbar));
    win.present();
}

fn list_body(infos: &[SignatureInfo]) -> gtk::Widget {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.set_margin_top(12);
    col.set_margin_bottom(12);
    col.set_margin_start(12);
    col.set_margin_end(12);

    let group = adw::PreferencesGroup::new();
    for info in infos {
        group.add(&signature_row(info));
    }
    col.append(&group);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&col));
    scroll.upcast()
}

fn signature_row(info: &SignatureInfo) -> adw::ActionRow {
    let severity = verify_severity(info);
    let row = adw::ActionRow::new();
    if let Some(name) = &info.name {
        row.set_title(&escape(name));
    } else {
        row.set_title(&escape(&format_verify_result(info)));
    }
    row.set_subtitle(&escape(&format_verify_result(info)));
    row.set_subtitle_lines(4);
    row.add_css_class(verify_css_class(severity));
    let icon = gtk::Image::from_icon_name(verify_icon(severity));
    icon.add_css_class(verify_css_class(severity));
    row.add_prefix(&icon);
    row
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
