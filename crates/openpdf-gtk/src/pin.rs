// SPDX-License-Identifier: MPL-2.0
//! GTK PIN dialog. Implements [`openpdf_core::PinProvider`] and returns
//! [`SecretPin`]. Prompt text comes from [`openpdf_core::pin_prompt_text`].
//! This crate never imports `openpdf-sign`.
//!
//! The PIN is [`Zeroizing<String>`] from the GTK entry, through the channel,
//! until [`SecretPin`]. The entry buffer is cleared immediately after read.
//! Cancel or an empty field is [`Error::PinCancelled`] (unit, no payload).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{pin_prompt_text, tr, Error, PinProvider, Result, SecretPin};
use zeroize::Zeroizing;

/// Modal PIN dialog. Stateless so the application binary can build it
/// before the viewer exists.
#[derive(Clone, Debug, Default)]
pub struct GtkPinProvider;

impl PinProvider for GtkPinProvider {
    fn request_pin(&self, token_label: &str) -> Result<SecretPin> {
        request_pin_blocking(&pin_prompt_text(token_label)).map(|pin| SecretPin::new(pin.as_str()))
    }
}

fn request_pin_blocking(prompt: &str) -> Result<Zeroizing<String>> {
    if !gtk::is_initialized() || gtk::gdk::Display::default().is_none() {
        return Err(Error::Sign("PIN dialog needs a display".into()));
    }
    let ctx = gtk::glib::MainContext::default();
    // Signing runs on a worker. Nested `iteration` on the GTK thread would
    // freeze the UI; refuse instead of re-entering the main loop.
    if ctx.is_owner() {
        return Err(Error::Sign(
            "PIN must be requested off the GTK thread".into(),
        ));
    }
    // Oneshot: the GTK thread sends exactly one Zeroizing<String> (or
    // PinCancelled). The worker blocks on recv; the GTK thread does not.
    let (tx, rx) = mpsc::sync_channel(1);
    let shown = Arc::new(AtomicBool::new(false));
    let flag = shown.clone();
    let prompt = prompt.to_string();
    ctx.invoke(move || {
        flag.store(true, Ordering::SeqCst);
        present_pin_dialog(&prompt, tx);
    });
    // Wait until the GTK thread actually runs the invoke. Unit tests and
    // shutdown have no loop, so do not `recv` forever.
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if shown.load(Ordering::SeqCst) {
            return rx.recv().unwrap_or_else(|_| Err(Error::PinCancelled));
        }
        if let Ok(result) = rx.try_recv() {
            return result;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if shown.load(Ordering::SeqCst) {
        return rx.recv().unwrap_or_else(|_| Err(Error::PinCancelled));
    }
    Err(Error::Sign("PIN dialog needs a display".into()))
}

fn take_pin(entry: &gtk::PasswordEntry) -> Zeroizing<String> {
    let pin = Zeroizing::new(entry.text().to_string());
    entry.set_text("");
    pin
}

fn submit_pin(send: &Rc<impl Fn(Result<Zeroizing<String>>)>, entry: &gtk::PasswordEntry) {
    let pin = take_pin(entry);
    if pin.is_empty() {
        send(Err(Error::PinCancelled));
    } else {
        send(Ok(pin));
    }
}

fn present_pin_dialog(prompt: &str, tx: SyncSender<Result<Zeroizing<String>>>) {
    let sent = Rc::new(Cell::new(false));
    let send = Rc::new(move |value: Result<Zeroizing<String>>| {
        if !sent.replace(true) {
            let _ = tx.send(value);
        }
    });

    let win = adw::Window::new();
    win.set_modal(true);
    win.set_default_size(380, 200);
    if let Some(app) = gtk::gio::Application::default() {
        if let Ok(app) = app.downcast::<gtk::Application>() {
            if let Some(parent) = app.active_window() {
                win.set_transient_for(Some(&parent));
            }
        }
    }
    win.set_title(Some(&tr("Enter PIN")));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let label = gtk::Label::new(Some(prompt));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label.add_css_class("dim-label");

    let entry = gtk::PasswordEntry::new();
    entry.set_show_peek_icon(true);
    entry.set_hexpand(true);
    entry.set_activates_default(true);

    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let unlock = gtk::Button::with_label(&tr("Unlock"));
    unlock.add_css_class("suggested-action");

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&unlock);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
    col.set_margin_top(16);
    col.set_margin_bottom(16);
    col.set_margin_start(16);
    col.set_margin_end(16);
    col.append(&label);
    col.append(&entry);
    col.append(&buttons);
    toolbar.set_content(Some(&col));
    win.set_content(Some(&toolbar));

    cancel.connect_clicked({
        let win = win.clone();
        let send = send.clone();
        move |_| {
            send(Err(Error::PinCancelled));
            win.close();
        }
    });
    unlock.connect_clicked({
        let win = win.clone();
        let entry = entry.clone();
        let send = send.clone();
        move |_| {
            submit_pin(&send, &entry);
            win.close();
        }
    });
    entry.connect_activate({
        let win = win.clone();
        let send = send.clone();
        move |entry| {
            submit_pin(&send, entry);
            win.close();
        }
    });
    win.connect_close_request(move |_| {
        send(Err(Error::PinCancelled));
        gtk::glib::Propagation::Proceed
    });

    win.present();
    entry.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_display_returns_error() {
        assert!(matches!(
            GtkPinProvider.request_pin("test"),
            Err(Error::Sign(_))
        ));
    }

    #[test]
    fn pin_cancelled_is_unit_variant() {
        assert!(matches!(Error::PinCancelled, Error::PinCancelled));
        assert!(!matches!(Error::PinCancelled, Error::Sign(_)));
    }

    #[test]
    fn take_pin_clears_the_entry_into_zeroizing() {
        let Some(()) = crate::test_gtk::on_gtk_thread(|| {
            let entry = gtk::PasswordEntry::new();
            entry.set_text("s3cret");
            let pin = take_pin(&entry);
            assert_eq!(pin.as_str(), "s3cret");
            assert_eq!(entry.text().as_str(), "");
        }) else {
            return;
        };
    }

    #[test]
    fn fake_signer_requests_pin_from_a_worker_thread() {
        use openpdf_core::{SignRequest, Signer};
        use std::time::Duration;

        struct Fake(GtkPinProvider);
        impl Signer for Fake {
            fn list_certificates(&self) -> Result<Vec<openpdf_core::CertificateInfo>> {
                Ok(Vec::new())
            }
            fn sign(&self, _pdf: &[u8], _request: &SignRequest) -> Result<Vec<u8>> {
                let pin = self.0.request_pin("test-token")?;
                Ok(pin.as_str().as_bytes().to_vec())
            }
            fn verify(&self, _pdf: &[u8]) -> Result<Vec<openpdf_core::SignatureInfo>> {
                Ok(Vec::new())
            }
        }

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Fake(GtkPinProvider).sign(b"%PDF", &SignRequest::new("test"));
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("PinProvider on a worker must not deadlock");
        assert!(
            matches!(result, Err(Error::Sign(_))),
            "worker PIN without a GTK loop must fail fast, got {result:?}"
        );
    }
}
