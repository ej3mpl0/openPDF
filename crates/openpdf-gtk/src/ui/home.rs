// SPDX-License-Identifier: MPL-2.0
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::i18n::t;
use crate::settings::Settings;
use crate::Lang;

pub struct HomePage {
    pub root: gtk::Widget,
}

pub fn build(
    settings: Rc<RefCell<Settings>>,
    on_open: impl Fn() + 'static,
    on_sample: impl Fn() + 'static,
    on_recent: impl Fn(PathBuf) + 'static,
    on_pending: impl Fn(&str) + 'static,
) -> HomePage {
    let lang = settings.borrow().lang();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(560);
    clamp.set_tightening_threshold(400);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 28);
    col.set_margin_top(56);
    col.set_margin_bottom(48);
    col.set_margin_start(24);
    col.set_margin_end(24);

    let hero = gtk::Box::new(gtk::Orientation::Vertical, 10);
    hero.set_halign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name("document-open-symbolic");
    icon.set_pixel_size(48);
    icon.add_css_class("accent");
    let title = gtk::Label::new(Some(t(lang, "openPDF")));
    title.add_css_class("title-1");
    title.set_justify(gtk::Justification::Center);
    let tagline = gtk::Label::new(Some(t(lang, "Fast, local PDF reader for Linux")));
    tagline.add_css_class("dim-label");
    tagline.set_justify(gtk::Justification::Center);
    tagline.set_wrap(true);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    actions.set_halign(gtk::Align::Center);
    actions.set_margin_top(8);
    let open_btn = gtk::Button::with_label(t(lang, "Open"));
    open_btn.add_css_class("suggested-action");
    open_btn.add_css_class("pill");
    open_btn.connect_clicked(move |_| on_open());
    let sample_btn = gtk::Button::with_label(t(lang, "Open sample document"));
    sample_btn.add_css_class("pill");
    sample_btn.connect_clicked(move |_| on_sample());
    actions.append(&open_btn);
    actions.append(&sample_btn);

    hero.append(&icon);
    hero.append(&title);
    hero.append(&tagline);
    hero.append(&actions);
    col.append(&hero);

    col.append(&recent_group(lang, &settings.borrow(), on_recent));
    col.append(&tools_group(lang, on_pending));

    clamp.set_child(Some(&col));
    scroll.set_child(Some(&clamp));
    outer.append(&scroll);

    HomePage {
        root: outer.upcast(),
    }
}

fn recent_group(
    lang: Lang,
    settings: &Settings,
    on_recent: impl Fn(PathBuf) + 'static,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(t(lang, "Recent files"));
    let on_recent = Rc::new(on_recent);
    if settings.recent.is_empty() {
        let row = adw::ActionRow::new();
        row.set_title(t(lang, "No recent files"));
        row.set_sensitive(false);
        group.add(&row);
    } else {
        for path in settings.recent.iter().take(8) {
            let row = adw::ActionRow::new();
            let name = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(path);
            row.set_title(name);
            row.set_subtitle(path);
            row.set_activatable(true);
            row.add_prefix(&gtk::Image::from_icon_name("x-office-document-symbolic"));
            let dest = PathBuf::from(path);
            let cb = on_recent.clone();
            row.connect_activated(move |_| cb(dest.clone()));
            group.add(&row);
        }
    }
    group
}

fn tools_group(lang: Lang, on_pending: impl Fn(&str) + 'static) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(t(lang, "Tools"));
    let on_pending = Rc::new(on_pending);

    let tools = [
        (t(lang, "View"), None),
        (t(lang, "Comment"), Some("Waiting on openpdf-core")),
        (
            t(lang, "Fill and sign"),
            Some("Signing goes through openpdf-core’s Signer trait (openpdf-sign). This UI never links that crate."),
        ),
        (t(lang, "Organize pages"), Some("Waiting on openpdf-core")),
        (t(lang, "Protect"), Some("Waiting on openpdf-core")),
        (t(lang, "Edit"), Some("Waiting on openpdf-core")),
    ];
    for (title, pending) in tools {
        let row = adw::ActionRow::new();
        row.set_title(title);
        row.set_activatable(true);
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let cb = on_pending.clone();
        if let Some(key) = pending {
            row.set_subtitle(t(lang, "Waiting on openpdf-core"));
            row.connect_activated(move |_| cb(t(lang, key)));
        } else {
            row.connect_activated(move |_| cb(t(lang, "Open sample document")));
        }
        group.add(&row);
    }
    group
}
