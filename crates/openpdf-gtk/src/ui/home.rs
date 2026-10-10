// SPDX-License-Identifier: MPL-2.0
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
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
    let on_open = Rc::new(on_open);
    let on_sample = Rc::new(on_sample);
    let on_recent = Rc::new(on_recent);
    let on_pending = Rc::new(on_pending);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(960);
    clamp.set_tightening_threshold(640);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 24);
    col.set_margin_top(40);
    col.set_margin_bottom(40);
    col.set_margin_start(24);
    col.set_margin_end(24);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    top.set_homogeneous(true);
    top.append(&drop_card(
        lang,
        on_open.clone(),
        on_sample.clone(),
        on_recent.clone(),
    ));
    top.append(&recent_card(lang, &settings.borrow(), on_recent.clone()));
    col.append(&top);

    let tools_title = gtk::Label::new(Some(t(lang, "Tools")));
    tools_title.add_css_class("title-3");
    tools_title.set_xalign(0.0);
    col.append(&tools_title);
    col.append(&tools_grid(lang, on_pending, on_sample));

    clamp.set_child(Some(&col));
    scroll.set_child(Some(&clamp));
    outer.append(&scroll);

    HomePage {
        root: outer.upcast(),
    }
}

fn drop_card(
    lang: Lang,
    on_open: Rc<impl Fn() + 'static>,
    on_sample: Rc<impl Fn() + 'static>,
    on_recent: Rc<impl Fn(PathBuf) + 'static>,
) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("drop-card");
    card.set_halign(gtk::Align::Fill);
    card.set_valign(gtk::Align::Fill);
    card.set_hexpand(true);

    let icon = gtk::Image::from_icon_name("document-open-symbolic");
    icon.set_pixel_size(40);
    icon.add_css_class("accent");
    icon.set_halign(gtk::Align::Center);
    icon.set_margin_top(8);

    let title = gtk::Label::new(Some(t(lang, "Drag a PDF here")));
    title.add_css_class("title-3");
    title.set_justify(gtk::Justification::Center);

    let hint = gtk::Label::new(Some(t(lang, "or open from your computer")));
    hint.add_css_class("dim-label");
    hint.set_justify(gtk::Justification::Center);

    let open_btn = gtk::Button::with_label(t(lang, "Open"));
    open_btn.add_css_class("suggested-action");
    open_btn.add_css_class("pill");
    open_btn.set_halign(gtk::Align::Center);
    open_btn.set_margin_top(8);
    let open_cb = on_open.clone();
    open_btn.connect_clicked(move |_| open_cb());

    let sample_btn = gtk::Button::with_label(t(lang, "Open sample document"));
    sample_btn.add_css_class("pill");
    sample_btn.set_halign(gtk::Align::Center);
    sample_btn.connect_clicked(move |_| on_sample());

    card.append(&icon);
    card.append(&title);
    card.append(&hint);
    card.append(&open_btn);
    card.append(&sample_btn);

    let drop = gtk::DropTarget::new(gio::File::static_type(), gtk::gdk::DragAction::COPY);
    drop.connect_drop(move |_, value, _, _| {
        if let Ok(file) = value.get::<gio::File>() {
            if let Some(path) = file.path() {
                on_recent(path);
                return true;
            }
        }
        false
    });
    card.add_controller(drop);
    card.upcast()
}

fn recent_card(
    lang: Lang,
    settings: &Settings,
    on_recent: Rc<impl Fn(PathBuf) + 'static>,
) -> gtk::Widget {
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 8);
    wrap.add_css_class("home-card");
    wrap.set_hexpand(true);

    let title = gtk::Label::new(Some(t(lang, "Recent files")));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    wrap.append(&title);

    if settings.recent.is_empty() {
        let empty = gtk::Label::new(Some(t(lang, "No recent files")));
        empty.add_css_class("dim-label");
        empty.set_xalign(0.0);
        wrap.append(&empty);
    } else {
        for path in settings.recent.iter().take(8) {
            let row = gtk::Button::new();
            row.add_css_class("flat");
            row.set_halign(gtk::Align::Fill);
            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            inner.append(&gtk::Image::from_icon_name("x-office-document-symbolic"));
            let name = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(path);
            let label = gtk::Label::new(Some(name));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            inner.append(&label);
            row.set_child(Some(&inner));
            let dest = PathBuf::from(path);
            let cb = on_recent.clone();
            row.connect_clicked(move |_| cb(dest.clone()));
            wrap.append(&row);
        }
    }
    wrap.upcast()
}

fn tools_grid(
    lang: Lang,
    on_pending: Rc<impl Fn(&str) + 'static>,
    on_sample: Rc<impl Fn() + 'static>,
) -> gtk::Widget {
    let flow = gtk::FlowBox::new();
    flow.set_min_children_per_line(2);
    flow.set_max_children_per_line(4);
    flow.set_column_spacing(12);
    flow.set_row_spacing(12);
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);

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
        let btn = gtk::Button::new();
        btn.add_css_class("tool-card");
        btn.add_css_class("flat");
        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let name = gtk::Label::new(Some(title));
        name.add_css_class("heading");
        name.set_xalign(0.0);
        col.append(&name);
        if pending.is_some() {
            let sub = gtk::Label::new(Some(t(lang, "Waiting on openpdf-core")));
            sub.add_css_class("dim-label");
            sub.add_css_class("caption");
            sub.set_xalign(0.0);
            sub.set_wrap(true);
            col.append(&sub);
        }
        btn.set_child(Some(&col));
        let cb = on_pending.clone();
        let sample = on_sample.clone();
        if let Some(key) = pending {
            btn.connect_clicked(move |_| cb(t(lang, key)));
        } else {
            btn.connect_clicked(move |_| sample());
        }
        flow.append(&btn);
    }
    flow.upcast()
}
