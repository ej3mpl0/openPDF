// SPDX-License-Identifier: MPL-2.0
use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use crate::i18n::t;
use crate::settings::{self, PageLayoutMode, Settings, Theme, ZoomMode};
use crate::Lang;

use super::apply_theme;

pub fn present(parent: &impl IsA<gtk::Window>, settings: Rc<RefCell<Settings>>) {
    let lang = settings.borrow().lang();
    let win = adw::PreferencesWindow::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_search_enabled(true);
    win.add(&general_page(lang, settings.clone()));
    win.add(&view_page(lang, settings.clone()));
    win.add(&advanced_page(lang, settings));
    win.present();
}

fn persist(settings: &Rc<RefCell<Settings>>) {
    let s = settings.borrow();
    let _ = settings::save(&s);
}

fn general_page(lang: Lang, settings: Rc<RefCell<Settings>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title(t(lang, "General"));
    page.set_icon_name(Some("preferences-system-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title(t(lang, "General"));

    let lang_row = adw::ComboRow::new();
    lang_row.set_title(t(lang, "Language"));
    lang_row.set_subtitle(t(lang, "Language applies the next time the window opens."));
    lang_row.set_model(Some(&gtk::StringList::new(&[
        t(lang, "System"),
        t(lang, "English"),
        t(lang, "Spanish"),
    ])));
    lang_row.set_selected(match settings.borrow().language.as_str() {
        "es" | "es-ES" | "es_ES" => 2,
        "en" | "en-US" | "en_US" => 1,
        _ => 0,
    });
    lang_row.connect_selected_notify({
        let settings = settings.clone();
        move |row| {
            settings.borrow_mut().language = match row.selected() {
                2 => "es".into(),
                1 => "en".into(),
                _ => "auto".into(),
            };
            persist(&settings);
        }
    });
    group.add(&lang_row);

    let theme_row = adw::ComboRow::new();
    theme_row.set_title(t(lang, "Theme"));
    theme_row.set_model(Some(&gtk::StringList::new(&[
        t(lang, "System"),
        t(lang, "Light"),
        t(lang, "Dark"),
    ])));
    theme_row.set_selected(match settings.borrow().theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    });
    theme_row.connect_selected_notify({
        let settings = settings.clone();
        move |row| {
            let theme = match row.selected() {
                1 => Theme::Light,
                2 => Theme::Dark,
                _ => Theme::System,
            };
            settings.borrow_mut().theme = theme;
            apply_theme(theme);
            persist(&settings);
        }
    });
    group.add(&theme_row);

    let restore = adw::SwitchRow::new();
    restore.set_title(t(lang, "Restore last session"));
    restore.set_active(settings.borrow().restore_last);
    restore.connect_active_notify({
        let settings = settings.clone();
        move |row| {
            settings.borrow_mut().restore_last = row.is_active();
            persist(&settings);
        }
    });
    group.add(&restore);

    page.add(&group);
    page
}

fn view_page(lang: Lang, settings: Rc<RefCell<Settings>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title(t(lang, "Viewing"));
    page.set_icon_name(Some("view-paged-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title(t(lang, "Viewing"));

    let thumbs = adw::SwitchRow::new();
    thumbs.set_title(t(lang, "Show thumbnail sidebar"));
    thumbs.set_active(settings.borrow().show_thumbnails);
    thumbs.connect_active_notify({
        let settings = settings.clone();
        move |row| {
            settings.borrow_mut().show_thumbnails = row.is_active();
            persist(&settings);
        }
    });
    group.add(&thumbs);

    let layout = adw::ComboRow::new();
    layout.set_title(t(lang, "Page layout"));
    layout.set_model(Some(&gtk::StringList::new(&[
        t(lang, "Continuous"),
        t(lang, "Single page"),
    ])));
    layout.set_selected(match settings.borrow().page_layout {
        PageLayoutMode::Continuous => 0,
        PageLayoutMode::Single => 1,
    });
    layout.connect_selected_notify({
        let settings = settings.clone();
        move |row| {
            settings.borrow_mut().page_layout = if row.selected() == 1 {
                PageLayoutMode::Single
            } else {
                PageLayoutMode::Continuous
            };
            persist(&settings);
        }
    });
    group.add(&layout);

    let zoom = adw::ComboRow::new();
    zoom.set_title(t(lang, "Default zoom"));
    zoom.set_model(Some(&gtk::StringList::new(&[
        t(lang, "Fit width"),
        t(lang, "Fit page"),
        "100%",
        "125%",
        "150%",
        "200%",
    ])));
    zoom.set_selected(
        match (settings.borrow().zoom_mode, settings.borrow().zoom_percent) {
            (ZoomMode::FitWidth, _) => 0,
            (ZoomMode::FitPage, _) => 1,
            (ZoomMode::Percent, 125) => 3,
            (ZoomMode::Percent, 150) => 4,
            (ZoomMode::Percent, 200) => 5,
            _ => 2,
        },
    );
    zoom.connect_selected_notify({
        let settings = settings.clone();
        move |row| {
            let mut s = settings.borrow_mut();
            match row.selected() {
                0 => s.zoom_mode = ZoomMode::FitWidth,
                1 => s.zoom_mode = ZoomMode::FitPage,
                3 => {
                    s.zoom_mode = ZoomMode::Percent;
                    s.zoom_percent = 125;
                }
                4 => {
                    s.zoom_mode = ZoomMode::Percent;
                    s.zoom_percent = 150;
                }
                5 => {
                    s.zoom_mode = ZoomMode::Percent;
                    s.zoom_percent = 200;
                }
                _ => {
                    s.zoom_mode = ZoomMode::Percent;
                    s.zoom_percent = 100;
                }
            }
            drop(s);
            persist(&settings);
        }
    });
    group.add(&zoom);

    page.add(&group);
    page
}

fn advanced_page(lang: Lang, settings: Rc<RefCell<Settings>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title(t(lang, "Advanced"));
    page.set_icon_name(Some("applications-engineering-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title(t(lang, "Advanced"));
    group.set_description(Some(t(
        lang,
        "256 px tiles and a cache cap keep RAM low on large PDFs.",
    )));

    let tile = adw::SpinRow::with_range(64.0, 512.0, 64.0);
    tile.set_title(t(lang, "Tile size (px)"));
    tile.set_value(f64::from(settings.borrow().tile_size));
    tile.connect_value_notify({
        let settings = settings.clone();
        move |row| {
            settings.borrow_mut().tile_size = row.value() as u32;
            persist(&settings);
        }
    });
    group.add(&tile);

    let cache = adw::SpinRow::with_range(16.0, 256.0, 16.0);
    cache.set_title(t(lang, "Tile cache (MiB)"));
    cache.set_value((settings.borrow().cache_bytes / (1024 * 1024)) as f64);
    cache.connect_value_notify({
        let settings = settings.clone();
        move |row| {
            settings.borrow_mut().cache_bytes = (row.value() as usize) * 1024 * 1024;
            persist(&settings);
        }
    });
    group.add(&cache);

    page.add(&group);
    page
}
