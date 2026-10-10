// SPDX-License-Identifier: MPL-2.0
use std::rc::Rc;
use std::sync::Arc;

use crate::i18n::{format_core_error, t};
use crate::sign::{certificate_can_sign, describe_certificate, sign_request_for};
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
        "Choose a signing certificate. The token will ask for the PIN if needed.",
    )));
    note.add_css_class("dim-label");
    note.set_wrap(true);
    note.set_margin_top(12);
    note.set_margin_start(12);
    note.set_margin_end(12);
    let group = adw::PreferencesGroup::new();
    for cert in certs {
        group.add(&certificate_row(lang, cert, win, on_pick.clone()));
    }
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.append(&note);
    col.append(&group);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_min_content_height(280);
    scroll.set_child(Some(&col));
    toolbar.set_content(Some(&scroll));
}

fn certificate_row(
    lang: Lang,
    cert: CertificateInfo,
    win: &adw::Window,
    on_pick: Rc<impl Fn(SignRequest) + 'static>,
) -> adw::ActionRow {
    let can_sign = certificate_can_sign(&cert);
    let row = adw::ActionRow::new();
    row.set_title(&escape(&certificate_cn(&cert.subject)));
    let mut subtitle = describe_certificate_local(&cert);
    if !can_sign {
        subtitle.push_str(" · ");
        subtitle.push_str(t(lang, "Not a signing certificate"));
    }
    row.set_subtitle(&escape(&subtitle));
    row.set_sensitive(can_sign);
    row.set_activatable(can_sign);
    if can_sign {
        let win_c = win.clone();
        row.connect_activated(move |_| {
            on_pick(sign_request_for(cert.id.clone()));
            win_c.close();
        });
    }
    row
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

/// Row title: holder's CN, not the raw DN. Core `describe_certificate` still
/// owns the subtitle; we only peel `CN=` here until Jian extends the helper.
pub(crate) fn certificate_cn(subject: &str) -> String {
    for part in subject.split([',', '/']) {
        let part = part.trim();
        let rest = part
            .strip_prefix("CN=")
            .or_else(|| part.strip_prefix("cn="))
            .or_else(|| part.strip_prefix("CN ="));
        if let Some(cn) = rest {
            let cn = cn.trim();
            if !cn.is_empty() {
                return cn.to_string();
            }
        }
    }
    subject.trim().to_string()
}

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
    describe_certificate(&info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_cn_takes_the_common_name() {
        assert_eq!(
            certificate_cn("O=AC DNIE,CN=María García (FIRMA),C=ES"),
            "María García (FIRMA)"
        );
        assert_eq!(certificate_cn("CN=CITIZEN (SIGNING)"), "CITIZEN (SIGNING)");
        assert_eq!(certificate_cn("no-cn-here"), "no-cn-here");
    }
}
