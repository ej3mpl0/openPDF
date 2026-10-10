// SPDX-License-Identifier: MPL-2.0
//! Engine bind/open/refresh must not run on the GTK main thread.
//!
//! `Engine::bind_bytes` blocks on `rx.recv()` while the render worker may
//! still be doing the synchronous font warmup. Call [`spawn_engine_bind`]
//! (gio thread pool → glib main context) and show [`present_busy`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;

use crate::Lang;

/// Extra milliseconds injected into [`spawn_engine_bind`] (tests only).
pub static TEST_SLOW_BIND_MS: AtomicU64 = AtomicU64::new(0);

/// Panic in debug builds if this thread owns the default glib main context.
pub fn assert_off_gtk_thread() {
    debug_assert!(
        !glib::MainContext::default().is_owner(),
        "Engine::bind/open/refresh must not run on the GTK main thread"
    );
}

fn maybe_test_slow_bind() {
    let ms = TEST_SLOW_BIND_MS.load(Ordering::SeqCst);
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Run `work` on the Gio blocking pool; `done` on the GTK main context.
///
/// The worker calls [`assert_off_gtk_thread`] so a mistaken inline bind
/// fails the debug build instead of freezing the UI.
pub fn spawn_engine_bind<T, W, D>(work: W, done: D)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    D: FnOnce(T) + 'static,
{
    glib::spawn_future_local(async move {
        match gio::spawn_blocking(move || {
            assert_off_gtk_thread();
            maybe_test_slow_bind();
            work()
        })
        .await
        {
            Ok(value) => done(value),
            Err(_) => {}
        }
    });
}

/// Modal spinner used for open / rebind / tool apply (same chrome as Signing…).
pub fn present_busy(parent: &impl IsA<gtk::Window>, lang: Lang, title: &str) -> adw::Window {
    let win = adw::Window::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_default_size(360, 180);
    win.set_title(Some(title));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    let spinner = gtk::Spinner::new();
    spinner.start();
    let status = adw::StatusPage::new();
    status.set_title(title);
    let _ = lang;
    status.set_child(Some(&spinner));
    toolbar.set_content(Some(&status));
    win.set_content(Some(&toolbar));
    win.present();
    win
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn slow_bind_keeps_main_loop_dispatching() {
        let ctx = glib::MainContext::default();
        let Ok(_acquire) = ctx.acquire() else {
            return;
        };
        TEST_SLOW_BIND_MS.store(200, Ordering::SeqCst);

        let dispatched = Rc::new(Cell::new(false));
        let finished = Rc::new(Cell::new(false));
        let overlap = Rc::new(Cell::new(false));

        glib::timeout_add_local(Duration::from_millis(40), {
            let dispatched = dispatched.clone();
            let finished = finished.clone();
            let overlap = overlap.clone();
            move || {
                dispatched.set(true);
                if !finished.get() {
                    overlap.set(true);
                }
                glib::ControlFlow::Break
            }
        });

        spawn_engine_bind(|| (), {
            let finished = finished.clone();
            move |()| finished.set(true)
        });

        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(2) && !finished.get() {
            let _ = ctx.iteration(true);
        }
        TEST_SLOW_BIND_MS.store(0, Ordering::SeqCst);

        assert!(
            dispatched.get(),
            "glib timeout must fire while bind is on the worker"
        );
        assert!(finished.get(), "slow bind must finish");
        assert!(
            overlap.get(),
            "timeout must run before spawn_engine_bind completes"
        );
    }
}
