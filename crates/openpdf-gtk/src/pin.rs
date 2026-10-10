//! GTK PIN dialog. Implements [`openpdf_core::PinProvider`] and returns
//! [`SecretPin`]. Prompt text comes from [`openpdf_core::pin_prompt_text`].
//! This crate never imports `openpdf-sign`.
//!
//! The PIN is [`Zeroizing<String>`] from the GTK entry, through the channel,
//! until [`SecretPin`]. The entry buffer is cleared immediately after read.
//! Cancel or an empty field is [`Error::PinCancelled`] (unit, no payload).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{
    describe_pin_status, pin_prompt_text, tr, Error, PinProvider, Result, SecretPin, TokenPinStatus,
};
use zeroize::Zeroizing;

/// Modal PIN dialog. Stateless so the application binary can build it
/// before the viewer exists.
#[derive(Clone, Debug, Default)]
pub struct GtkPinProvider;

static PENDING_PIN_STATUS: Mutex<Option<TokenPinStatus>> = Mutex::new(None);

/// Stash PKCS#11 PIN flags from the picker so the dialog can warn.
pub fn set_pending_pin_status(status: Option<TokenPinStatus>) {
    if let Ok(mut slot) = PENDING_PIN_STATUS.lock() {
        *slot = status;
    }
}

fn take_pending_pin_status() -> Option<TokenPinStatus> {
    PENDING_PIN_STATUS
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
}

impl PinProvider for GtkPinProvider {
    fn request_pin(&self, token_label: &str) -> Result<SecretPin> {
        let warning = take_pending_pin_status().and_then(|s| describe_pin_status(&s));
        request_pin_blocking(&pin_prompt_text(token_label), warning.as_deref())
            .map(|pin| SecretPin::new(pin.as_str()))
    }
}

fn request_pin_blocking(prompt: &str, warning: Option<&str>) -> Result<Zeroizing<String>> {
    if !gtk::is_initialized() {
        return Err(Error::PinRequired);
    }
    let (tx, rx) = mpsc::channel();
    let prompt = prompt.to_string();
    let launch = {
        let tx = tx.clone();
        let warning = warning.map(str::to_string);
        move || present_pin_dialog(&prompt, warning.as_deref(), tx)
    };
    let ctx = gtk::glib::MainContext::default();
    if ctx.is_owner() {
        launch();
        loop {
            if let Ok(result) = rx.try_recv() {
                return result;
            }
            ctx.iteration(true);
        }
    }
    ctx.invoke(launch);
    rx.recv().unwrap_or_else(|_| Err(Error::PinCancelled))
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

fn present_pin_dialog(
    prompt: &str,
    warning: Option<&str>,
    tx: Sender<Result<Zeroizing<String>>>,
) {
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
    if let Some(warn) = warning {
        let banner = gtk::Label::new(Some(warn));
        banner.set_wrap(true);
        banner.set_xalign(0.0);
        banner.add_css_class("warning");
        col.append(&banner);
    }
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
            Err(Error::PinRequired)
        ));
    }

    #[test]
    fn pin_cancelled_is_unit_variant() {
        assert!(matches!(Error::PinCancelled, Error::PinCancelled));
        assert!(!matches!(Error::PinCancelled, Error::Sign(_)));
    }

    #[test]
    fn take_pin_clears_the_entry_into_zeroizing() {
        if gtk::init().is_err() && !gtk::is_initialized() {
            return;
        }
        let entry = gtk::PasswordEntry::new();
        entry.set_text("s3cret");
        let pin = take_pin(&entry);
        assert_eq!(pin.as_str(), "s3cret");
        assert_eq!(entry.text().as_str(), "");
    }
}
