//! GTK PIN dialog. Implements [`openpdf_core::PinProvider`] and returns
//! [`SecretPin`]. Prompt text comes from [`openpdf_core::pin_prompt_text`].
//! This crate never imports `openpdf-sign`.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{pin_prompt_text, tr, Error, PinProvider, Result, SecretPin};

/// Modal PIN dialog. Stateless so the application binary can build it
/// before the viewer exists.
#[derive(Clone, Debug, Default)]
pub struct GtkPinProvider;

impl PinProvider for GtkPinProvider {
    fn request_pin(&self, token_label: &str) -> Result<SecretPin> {
        request_pin_blocking(&pin_prompt_text(token_label)).map(SecretPin::new)
    }
}

fn request_pin_blocking(prompt: &str) -> Result<String> {
    if !gtk::is_initialized() {
        return Err(Error::Sign("PIN dialog needs a display".into()));
    }
    let (tx, rx) = mpsc::channel();
    let prompt = prompt.to_string();
    let launch = {
        let tx = tx.clone();
        move || present_pin_dialog(&prompt, tx)
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
    rx.recv()
        .unwrap_or_else(|_| Err(Error::Sign("PIN entry cancelled".into())))
}

fn present_pin_dialog(prompt: &str, tx: Sender<Result<String>>) {
    let sent = Rc::new(Cell::new(false));
    let send = Rc::new(move |value: Result<String>| {
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
            send(Err(Error::Sign("PIN entry cancelled".into())));
            win.close();
        }
    });
    unlock.connect_clicked({
        let win = win.clone();
        let entry = entry.clone();
        let send = send.clone();
        move |_| {
            send(Ok(entry.text().to_string()));
            win.close();
        }
    });
    entry.connect_activate({
        let win = win.clone();
        let send = send.clone();
        move |entry| {
            send(Ok(entry.text().to_string()));
            win.close();
        }
    });
    win.connect_close_request(move |_| {
        send(Err(Error::Sign("PIN entry cancelled".into())));
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
}
