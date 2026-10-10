// SPDX-License-Identifier: MPL-2.0
//! One dedicated thread owns `gtk::init` for unit tests.
//!
//! `cargo test` default `--test-threads` is parallel. GTK/GDK objects are
//! main-thread-only. Two tests calling `gtk::init` from different threads
//! panic (`Attempted to initialize GTK from two different threads`) or
//! SIGSEGV under GSK ngl. Every GTK-touching test must run its body through
//! [`on_gtk_thread`].

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;

use gtk4 as gtk;

type Job = Box<dyn FnOnce() + Send>;

static TX: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();

fn sender() -> &'static Mutex<mpsc::Sender<Job>> {
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("openpdf-gtk-tests".into())
            .spawn(move || {
                let _ = gtk::init();
                let ctx = gtk::glib::MainContext::default();
                let _acquire = ctx.acquire();
                while let Ok(job) = rx.recv() {
                    job();
                    while ctx.iteration(false) {}
                }
            })
            .expect("start GTK test thread");
        Mutex::new(tx)
    })
}

/// Run `f` on the process-wide GTK test thread. Assertion panics resume here.
///
/// Returns `None` if GTK could not init on that thread (no display).
pub fn on_gtk_thread<F, R>(f: F) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (rtx, rrx) = mpsc::sync_channel(1);
    let job: Job = Box::new(move || {
        let out = catch_unwind(AssertUnwindSafe(|| {
            if !gtk::is_initialized_main_thread() {
                return None;
            }
            Some(f())
        }));
        let _ = rtx.send(out);
    });
    sender().lock().ok()?.send(job).ok()?;
    match rrx.recv() {
        Ok(Ok(v)) => v,
        Ok(Err(payload)) => resume_unwind(payload),
        Err(_) => None,
    }
}
