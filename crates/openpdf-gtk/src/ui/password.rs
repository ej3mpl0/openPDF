// SPDX-License-Identifier: MPL-2.0
//! Password prompt for [`Error::PasswordRequired`] / [`Error::WrongPassword`].

use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::Error;
use zeroize::Zeroizing;

use crate::i18n::t;
use crate::Lang;

/// Modal password dialog. The field is read into [`Zeroizing`] and then
/// cleared so a previous attempt cannot be reused. Wrong password keeps the
/// dialog open with an error. Cancel is silent.
pub fn prompt(
    parent: &impl IsA<gtk::Window>,
    lang: Lang,
    path: &Path,
    attempt: impl Fn(Zeroizing<String>) -> Result<(), Error> + 'static,
) {
    let win = adw::Window::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_default_size(420, 180);
    win.set_title(Some(t(lang, "Password required")));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let group = adw::PreferencesGroup::new();
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("PDF");
    group.set_description(Some(&openpdf_core::tr_args(
        "Enter the password for «{file}»",
        &[("file", name)],
    )));

    let row = adw::PasswordEntryRow::new();
    row.set_title(t(lang, "Password"));
    group.add(&row);

    let err = gtk::Label::new(None);
    err.add_css_class("error");
    err.add_css_class("dim-label");
    err.set_wrap(true);
    err.set_xalign(0.0);
    err.set_visible(false);

    let unlock = gtk::Button::with_label(t(lang, "Unlock"));
    unlock.add_css_class("suggested-action");
    unlock.add_css_class("pill");
    let cancel = gtk::Button::with_label(t(lang, "Cancel"));
    cancel.add_css_class("pill");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(12);
    buttons.append(&cancel);
    buttons.append(&unlock);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.set_margin_top(16);
    col.set_margin_bottom(16);
    col.set_margin_start(16);
    col.set_margin_end(16);
    col.append(&group);
    col.append(&err);
    col.append(&buttons);
    toolbar.set_content(Some(&col));
    win.set_content(Some(&toolbar));

    let win_c = win.clone();
    cancel.connect_clicked(move |_| {
        win_c.close();
    });

    let submit = Rc::new({
        let row = row.clone();
        let err = err.clone();
        let win = win.clone();
        move || {
            let pw = Zeroizing::new(row.text().to_string());
            row.set_text("");
            match attempt(pw) {
                Ok(()) => win.close(),
                Err(Error::WrongPassword) => {
                    err.set_text(t(lang, "wrong password"));
                    err.set_visible(true);
                }
                Err(Error::PasswordRequired) => {
                    err.set_text(t(lang, "password is required"));
                    err.set_visible(true);
                }
                Err(e) => {
                    err.set_text(&e.to_string());
                    err.set_visible(true);
                }
            }
        }
    });
    unlock.connect_clicked({
        let submit = submit.clone();
        move |_| submit()
    });
    row.connect_entry_activated({
        let submit = submit.clone();
        move |_| submit()
    });

    win.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpdf_core::Error;

    #[test]
    fn wrong_password_is_structured_not_sign() {
        assert!(matches!(Error::WrongPassword, Error::WrongPassword));
        assert!(!matches!(Error::WrongPassword, Error::Sign(_)));
        assert!(matches!(Error::PasswordRequired, Error::PasswordRequired));
    }

    #[test]
    fn zeroizing_read_is_cleared_from_plain_string() {
        let pw = Zeroizing::new(String::from("secret"));
        assert_eq!(pw.as_str(), "secret");
        drop(pw);
    }
}
