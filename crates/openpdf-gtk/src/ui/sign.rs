// SPDX-License-Identifier: MPL-2.0
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::i18n::{format_core_error, t};
use crate::sign::{
    certificate_can_sign, certificate_common_name, describe_certificate_detail, sign_request_for,
};
use crate::Lang;
use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{CertificateInfo, Error, SignRequest, Signer, SignerUnavailableReason};

use super::show_toast;

pub fn present_picker(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    signer: Arc<dyn Signer>,
    on_pick: impl Fn(SignRequest) + 'static,
) {
    let on_pick = Rc::new(on_pick);
    let win = adw::Window::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_default_size(520, 560);
    win.set_title(Some(t(lang, "Sign with certificate")));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let spinner = gtk::Spinner::new();
    spinner.start();
    let status = adw::StatusPage::new();
    status.set_icon_name(Some("dialog-password-symbolic"));
    status.set_title(t(lang, "Sign with certificate"));
    status.set_description(Some(t(lang, "Loading certificates…")));
    status.set_child(Some(&spinner));
    toolbar.set_content(Some(&status));

    win.set_content(Some(&toolbar));
    win.present();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(signer.list_certificates());
    });

    let overlay = overlay.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        match rx.try_recv() {
            Ok(Ok(certs)) => {
                show_certificate_list(&toolbar, &overlay, lang, certs, &win, on_pick.clone());
                glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                show_list_error(&toolbar, &overlay, lang, &format_core_error(&err));
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                show_list_error(
                    &toolbar,
                    &overlay,
                    lang,
                    &format_core_error(&Error::signer_unavailable(
                        SignerUnavailableReason::load_failed("", "certificate list channel closed"),
                    )),
                );
                glib::ControlFlow::Break
            }
        }
    });
}

fn show_list_error(toolbar: &adw::ToolbarView, overlay: &adw::ToastOverlay, lang: Lang, msg: &str) {
    let status = adw::StatusPage::new();
    status.set_icon_name(Some("dialog-password-symbolic"));
    status.set_title(t(lang, "Sign with certificate"));
    status.set_description(Some(msg));
    toolbar.set_content(Some(&status));
    show_toast(overlay, msg);
}

fn show_certificate_list(
    toolbar: &adw::ToolbarView,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    certs: Vec<CertificateInfo>,
    win: &adw::Window,
    on_pick: Rc<impl Fn(SignRequest) + 'static>,
) {
    if certs.is_empty() {
        let status = adw::StatusPage::new();
        status.set_icon_name(Some("dialog-password-symbolic"));
        status.set_title(t(lang, "Sign with certificate"));
        status.set_description(Some(t(lang, "No certificates found.")));
        toolbar.set_content(Some(&status));
        show_toast(overlay, t(lang, "No certificates found."));
        return;
    }
    let note = gtk::Label::new(Some(t(
        lang,
        "Certificates stay on this computer. openPDF does not send them anywhere.",
    )));
    note.add_css_class("dim-label");
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.set_margin_top(16);
    note.set_margin_start(16);
    note.set_margin_end(16);

    let pin_note = gtk::Label::new(Some(t(
        lang,
        "Choose a signing certificate. The token will ask for the PIN if needed.",
    )));
    pin_note.add_css_class("dim-label");
    pin_note.set_wrap(true);
    pin_note.set_xalign(0.0);
    pin_note.set_margin_start(16);
    pin_note.set_margin_end(16);

    let selected: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    list.set_margin_start(16);
    list.set_margin_end(16);
    list.set_margin_top(12);

    let mut first_radio: Option<gtk::CheckButton> = None;
    for cert in certs {
        let can_sign = certificate_can_sign(&cert);
        let radio = gtk::CheckButton::new();
        if let Some(first) = &first_radio {
            radio.set_group(Some(first));
        } else {
            first_radio = Some(radio.clone());
        }
        let row = adw::ActionRow::new();
        row.add_css_class("cert-row");
        row.set_title(&escape(&certificate_common_name(&cert.subject)));
        let mut subtitle = describe_certificate_local(&cert);
        if !can_sign {
            subtitle.push_str(" · ");
            subtitle.push_str(t(lang, "Not a signing certificate"));
        }
        row.set_subtitle(&escape(&subtitle));
        row.set_sensitive(can_sign);
        row.set_activatable(can_sign);
        row.add_prefix(&radio);
        let radio_for_row = radio.clone();
        row.connect_activated(move |_| {
            radio_for_row.set_active(true);
        });
        let id = cert.id.clone();
        let selected_c = selected.clone();
        let row_c = row.clone();
        radio.set_sensitive(can_sign);
        radio.connect_toggled(move |btn| {
            if btn.is_active() {
                *selected_c.borrow_mut() = Some(id.clone());
                row_c.add_css_class("selected");
            } else {
                row_c.remove_css_class("selected");
            }
        });
        if can_sign && selected.borrow().is_none() {
            radio.set_active(true);
        }
        list.append(&row);
    }

    let copy_note = gtk::Label::new(Some(t(
        lang,
        "A signed copy is saved. The original is not modified.",
    )));
    copy_note.add_css_class("dim-label");
    copy_note.set_wrap(true);
    copy_note.set_xalign(0.0);
    copy_note.set_margin_start(16);
    copy_note.set_margin_end(16);
    copy_note.set_margin_top(8);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("sign-footer");
    footer.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label(t(lang, "Cancel"));
    cancel.add_css_class("pill");
    let win_cancel = win.clone();
    cancel.connect_clicked(move |_| {
        win_cancel.close();
    });
    let sign_btn = gtk::Button::with_label(t(lang, "Sign and save a copy"));
    sign_btn.add_css_class("suggested-action");
    sign_btn.add_css_class("pill");
    let win_sign = win.clone();
    let selected_sign = selected.clone();
    sign_btn.connect_clicked(move |_| {
        if let Some(id) = selected_sign.borrow().clone() {
            on_pick(sign_request_for(id));
            win_sign.close();
        }
    });
    footer.append(&cancel);
    footer.append(&sign_btn);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.add_css_class("cert-dialog");
    col.append(&note);
    col.append(&pin_note);
    col.append(&list);
    col.append(&copy_note);
    col.append(&footer);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(280);
    scroll.set_child(Some(&col));
    toolbar.set_content(Some(&scroll));
}

/// Sync helper for tests. The window signs on a worker via
/// [`crate::Opened::bytes_for_sign`] + [`Signer::sign`] +
/// [`crate::Opened::apply_signed_bytes`] so `C_Sign` never runs on the GTK thread.
#[allow(dead_code)]
pub fn try_sign(
    opened: &crate::Opened,
    request: &SignRequest,
    signer: &dyn Signer,
) -> Result<(), String> {
    opened
        .apply_signer(signer, request)
        .map_err(|e| e.to_string())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// RFC 3339 / ISO-8601 from the token → the user's locale and local time.
/// Core keeps the raw strings (UniFFI); glib owns `%x %X`.
fn format_cert_time(raw: &str) -> String {
    let raw = raw.trim();
    if let Ok(dt) = gtk::glib::DateTime::from_iso8601(raw, None) {
        if let Ok(local) = dt.to_local() {
            if let Ok(s) = local.format("%x %X") {
                return s.to_string();
            }
        }
    }
    raw.to_string()
}

fn describe_certificate_local(cert: &CertificateInfo) -> String {
    let mut info = cert.clone();
    if let Some(a) = info.validity.not_before.as_mut() {
        *a = format_cert_time(a);
    }
    if let Some(b) = info.validity.not_after.as_mut() {
        *b = format_cert_time(b);
    }
    describe_certificate_detail(&info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_title_is_the_common_name() {
        assert_eq!(
            certificate_common_name("O=AC DNIE,CN=Maria Garcia (FIRMA),C=ES"),
            "Maria Garcia (FIRMA)"
        );
    }

    #[test]
    fn format_cert_time_drops_rfc3339_z() {
        let formatted = format_cert_time("2020-01-01T00:00:00Z");
        assert!(
            !formatted.contains('Z'),
            "locale/local time must not keep the RFC3339 Z: {formatted}"
        );
        assert_ne!(formatted, "2020-01-01T00:00:00Z");
    }
}
