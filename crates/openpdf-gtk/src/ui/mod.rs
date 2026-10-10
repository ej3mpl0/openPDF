// SPDX-License-Identifier: MPL-2.0
mod about;
mod home;
mod password;
mod preferences;
mod sign;
mod tools;
mod viewer;
mod window;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::measure::{
    self, peak_rss_bytes, BudgetReport, DEFAULT_MEASURE_PAGES, MIN_THUMB_TILES, MIN_VISIBLE_TILES,
};
use crate::settings::{Settings, Theme};
use crate::sign::{NoopVerifier, UnavailableSigner};
use gtk::gio::ApplicationFlags;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{Signer, Verifier};

pub fn run(initial: Option<PathBuf>, signer: Box<dyn Signer>, verifier: Box<dyn Verifier>) {
    let signer = Arc::<dyn Signer>::from(signer);
    let verifier = Arc::<dyn Verifier>::from(verifier);
    let app = adw::Application::builder()
        .application_id(crate::APP_ID)
        .flags(ApplicationFlags::HANDLES_OPEN | ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_startup(|_| {
        adw::init().ok();
        register_icons();
        load_css();
    });

    let win_slot: Rc<RefCell<Option<window::OpenPdfWindow>>> = Rc::new(RefCell::new(None));
    let cli_path = Rc::new(RefCell::new(initial));

    app.connect_activate({
        let signer = signer.clone();
        let verifier = verifier.clone();
        let win_slot = win_slot.clone();
        let cli_path = cli_path.clone();
        move |app| {
            // Defaults + chrome only before present(). Settings, recent files, and
            // the home/tools widgets wait for the first idle so nothing blocks the
            // first frame (and nothing talks to a portal until the user opens a file).
            if win_slot.borrow().is_none() {
                let settings = Settings::default();
                settings.apply_locale();
                apply_theme(settings.theme);
                install_accels(app);
                let win =
                    window::OpenPdfWindow::new(app, settings, signer.clone(), verifier.clone());
                win.present();
                *win_slot.borrow_mut() = Some(win);
                if let Some(win) = win_slot.borrow().as_ref() {
                    let win_home = win.clone();
                    glib::idle_add_local_once(move || {
                        win_home.hydrate_settings();
                        win_home.ensure_home();
                    });
                }
            }
            if let Some(path) = cli_path.borrow_mut().take() {
                if let Some(win) = win_slot.borrow().as_ref() {
                    let win = win.clone();
                    glib::idle_add_local_full(glib::Priority::HIGH, move || {
                        win.open_path(&path);
                        glib::ControlFlow::Break
                    });
                }
            }
        }
    });

    app.connect_open(move |app, files, _hint| {
        // `openpdf-gtk FILE` is delivered here. Without HANDLES_OPEN, GApplication
        // exits with "This application can not open files".
        app.activate();
        if let Some(win) = win_slot.borrow().as_ref() {
            for file in files {
                if let Some(path) = file.path() {
                    win.open_path(&path);
                }
            }
        }
    });

    app.run();
}

/// Open the real libadwaita window, load a multi-page PDF, wait until visible
/// tiles and thumbnails are painted, then print peak RSS / startup and quit.
///
/// The window is presented before PDFium/document work so the first frame is
/// not blocked on the engine. `startup_ms` (alias `ui_all_visible_ms`) is
/// milliseconds from process start until every visible page tile and every
/// visible thumbnail has been painted. `ui_first_tile_ms` is the first painted
/// page tile; `ui_thumbs_done_ms` is when the thumbnail strip meets the gate.
/// Other `ui_*` marks are also milliseconds from process start.
pub fn run_measure(pages: usize) -> i32 {
    measure::mark_process_start();
    if !measure::has_display() {
        eprintln!("measure: {}", measure::display_hint());
        return 1;
    }
    measure::isolate_xdg();
    let pages = if pages == 0 {
        DEFAULT_MEASURE_PAGES
    } else {
        pages
    };
    let pdf = match measure::write_measure_pdf(pages) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("measure: {e}");
            return 1;
        }
    };

    let report: Rc<RefCell<Option<BudgetReport>>> = Rc::new(RefCell::new(None));
    let timed_out = Rc::new(Cell::new(false));
    let app = adw::Application::builder()
        .application_id(crate::MEASURE_APP_ID)
        .flags(ApplicationFlags::NON_UNIQUE)
        .build();

    let ui_adw_init_ms = Rc::new(Cell::new(0u128));
    let ui_css_ms = Rc::new(Cell::new(0u128));
    app.connect_startup({
        let ui_adw_init_ms = ui_adw_init_ms.clone();
        let ui_css_ms = ui_css_ms.clone();
        move |_| {
            measure::warmup_fontconfig_pango();
            measure::mark_pango_init();
            adw::init().ok();
            register_icons();
            ui_adw_init_ms.set(measure::process_elapsed_ms());
            load_css();
            ui_css_ms.set(measure::process_elapsed_ms());
        }
    });

    let report_c = report.clone();
    let timed_out_c = timed_out.clone();
    app.connect_activate(move |app| {
        let ui_activate_ms = measure::process_elapsed_ms();
        let mut settings = Settings::default();
        settings.restore_last = false;
        settings.show_thumbnails = true;
        settings.language = "en".into();
        settings.apply_locale();
        apply_theme(settings.theme);
        install_accels(app);
        let win = window::OpenPdfWindow::new(
            app,
            settings,
            Arc::new(UnavailableSigner),
            Arc::new(NoopVerifier),
        );
        let ui_window_build_ms = measure::process_elapsed_ms();
        win.present();
        let ui_window_present_ms = measure::process_elapsed_ms();
        let first_frame = Rc::new(Cell::new(0u128));
        attach_first_frame(win.widget(), first_frame.clone());
        let win_open = win.clone();
        let pdf_open = pdf.clone();
        glib::idle_add_local_full(glib::Priority::HIGH, move || {
            win_open.open_path(&pdf_open);
            glib::ControlFlow::Break
        });
        watch_measure(
            app,
            win,
            report_c.clone(),
            timed_out_c.clone(),
            MeasureMarks {
                ui_activate_ms,
                ui_window_present_ms,
                ui_adw_init_ms: ui_adw_init_ms.get(),
                ui_css_ms: ui_css_ms.get(),
                ui_window_build_ms,
                first_frame,
            },
        );
    });

    let _ = app.run_with_args::<&str>(&[]);
    if timed_out.get() {
        return 1;
    }
    let result = report.borrow().clone();
    match result {
        Some(r) => {
            print!("{r}");
            i32::from(!r.ok)
        }
        None => {
            eprintln!("measure: window quit before tiles were ready");
            1
        }
    }
}

struct MeasureMarks {
    ui_activate_ms: u128,
    ui_window_present_ms: u128,
    ui_adw_init_ms: u128,
    ui_css_ms: u128,
    ui_window_build_ms: u128,
    first_frame: Rc<Cell<u128>>,
}

fn attach_first_frame(widget: &impl IsA<gtk::Widget>, slot: Rc<Cell<u128>>) {
    widget.add_tick_callback(move |_, _| {
        if slot.get() == 0 {
            slot.set(measure::process_elapsed_ms());
        }
        glib::ControlFlow::Break
    });
}

fn watch_measure(
    app: &adw::Application,
    win: window::OpenPdfWindow,
    report: Rc<RefCell<Option<BudgetReport>>>,
    timed_out: Rc<Cell<bool>>,
    marks: MeasureMarks,
) {
    let app = app.clone();
    let peak = Rc::new(Cell::new(peak_rss_bytes()));
    let laid_out = Rc::new(Cell::new(false));
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        peak.set(peak.get().max(peak_rss_bytes()));
        let elapsed = measure::process_elapsed_ms();

        if let Some(view) = win.current_view() {
            if view.opened().is_placeholder() {
                finish_measure(
                    &app,
                    &report,
                    failed_report(
                        view.opened().page_count(),
                        elapsed,
                        peak.get(),
                        "Engine/PDFium is required; placeholder renderer is not measured",
                    ),
                );
                return glib::ControlFlow::Break;
            }
            if !laid_out.get() {
                view.prepare_measure_viewport();
                laid_out.set(true);
            }
            // Do not poll() here — draining the render queue would put every
            // tile in cache before the first draw and collapse first_tile
            // onto all_visible. The tile pump delivers one arrival at a time.
            let ready = view.tile_ready();
            if measure::ui_all_visible_painted_ms() == 0 {
                view.ensure_laid_out();
                view.queue_repaint();
            }
            // startup_ms is marked from the DrawingArea draw path, not here.
            // Do not finish on a stale mark from a tiny first allocation:
            // Xvfb often grows the viewport afterward and tile_ready drops
            // to 0/N until the new keys are painted.
            let startup_ms = measure::ui_all_visible_painted_ms();
            if laid_out.get()
                && view.viewport_allocated()
                && startup_ms > 0
                && measure::ui_first_tile_painted_ms() > 0
                && ready.visible_ready >= MIN_VISIBLE_TILES
                && ready.thumb_ready >= MIN_THUMB_TILES
            {
                finish_measure(
                    &app,
                    &report,
                    BudgetReport {
                        pages: view.opened().page_count(),
                        startup_ms,
                        rss_bytes: peak.get().max(peak_rss_bytes()),
                        visible_ready: ready.visible_ready,
                        visible_total: ready.visible_total,
                        thumb_ready: ready.thumb_ready,
                        thumb_total: ready.thumb_total,
                        ui_activate_ms: marks.ui_activate_ms,
                        ui_window_present_ms: marks.ui_window_present_ms,
                        ui_first_frame_ms: marks.first_frame.get(),
                        ui_first_tile_ms: measure::ui_first_tile_painted_ms(),
                        pdfium_load_ms: measure::ui_pdfium_load_ms(),
                        ui_pango_init_ms: measure::ui_pango_init_ms(),
                        pdfium_sha256: measure::pdfium_library_sha256(),
                        ui_adw_init_ms: marks.ui_adw_init_ms,
                        ui_css_ms: marks.ui_css_ms,
                        ui_window_build_ms: marks.ui_window_build_ms,
                        ui_doc_load_start_ms: measure::ui_doc_load_start_ms(),
                        ui_doc_loaded_ms: measure::ui_doc_loaded_ms(),
                        ui_first_tile_requested_ms: measure::ui_first_tile_requested_ms(),
                        ui_first_tile_rendered_ms: measure::ui_first_tile_rendered_ms(),
                        ui_first_surface_ready_ms: measure::ui_first_surface_ready_ms(),
                        ui_all_visible_ms: startup_ms,
                        ui_thumbs_done_ms: measure::ui_thumbs_done_ms(),
                        ok: false,
                        error: None,
                    }
                    .evaluate(),
                );
                return glib::ControlFlow::Break;
            }
        }

        let timeout_ms = measure::measure_timeout_ms();
        if elapsed >= timeout_ms {
            let (vr, vt, tr, tt) = match win.current_view() {
                Some(view) => {
                    let r = view.tile_ready();
                    (
                        r.visible_ready,
                        r.visible_total,
                        r.thumb_ready,
                        r.thumb_total,
                    )
                }
                None => (0, 0, 0, 0),
            };
            eprintln!(
                "{}",
                measure::format_measure_timeout(timeout_ms, vr, vt, tr, tt)
            );
            timed_out.set(true);
            app.quit();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn failed_report(pages: usize, startup_ms: u128, rss_bytes: u64, error: &str) -> BudgetReport {
    BudgetReport {
        pages,
        startup_ms,
        rss_bytes,
        error: Some(error.to_string()),
        ..Default::default()
    }
}

fn finish_measure(
    app: &adw::Application,
    slot: &RefCell<Option<BudgetReport>>,
    report: BudgetReport,
) {
    *slot.borrow_mut() = Some(report);
    app.quit();
}

pub(crate) fn apply_theme(theme: Theme) {
    let mgr = adw::StyleManager::default();
    mgr.set_color_scheme(match theme {
        Theme::System => adw::ColorScheme::Default,
        Theme::Light => adw::ColorScheme::ForceLight,
        Theme::Dark => adw::ColorScheme::ForceDark,
    });
}

fn install_accels(app: &adw::Application) {
    app.set_accels_for_action("app.open", &["<Ctrl>o"]);
    app.set_accels_for_action("app.sign", &["<Ctrl>Return"]);
    app.set_accels_for_action("app.preferences", &["<Ctrl>comma"]);
    app.set_accels_for_action("app.about", &["<Ctrl>question"]);
    app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
    app.set_accels_for_action("win.zoom-in", &["<Ctrl>plus", "<Ctrl>equal"]);
    app.set_accels_for_action("win.zoom-out", &["<Ctrl>minus"]);
    app.set_accels_for_action("win.toggle-sidebar", &["<Ctrl>b", "F9"]);
    app.set_accels_for_action("win.find", &["<Ctrl>f"]);
    app.set_accels_for_action("win.next-page", &["Page_Down"]);
    app.set_accels_for_action("win.prev-page", &["Page_Up"]);
    app.set_accels_for_action("win.rotate", &["<Ctrl>r"]);
    app.set_accels_for_action("win.undo", &["<Ctrl>z"]);
    app.set_accels_for_action("win.redo", &["<Ctrl><Shift>z"]);
}

fn register_icons() {
    // `--measure` skips the bundled GResource so peak RSS stays comparable
    // across boxes (Adwaita + every theme walk is the usual 15–20 MiB delta).
    if gtk::gio::Application::default()
        .and_then(|app| app.application_id())
        .is_some_and(|id| id == crate::MEASURE_APP_ID)
    {
        return;
    }
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icons.gresource"));
    match gtk::gio::Resource::from_data(&gtk::glib::Bytes::from_static(bytes)) {
        Ok(resource) => gtk::gio::resources_register(&resource),
        Err(err) => eprintln!("openpdf-gtk: icons GResource: {err}"),
    }
    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        theme.add_resource_path("/io/github/ej3mpl0/openpdf/icons");
        // Do not call set_theme_name on the display singleton (GTK 4 asserts).
        // add_resource_path is enough for the bundled header icons; Adwaita
        // remains the default theme so GTK does not walk every installed theme.
    }
}

fn load_css() {
    if gtk::gio::Application::default()
        .and_then(|app| app.application_id())
        .is_some_and(|id| id == crate::MEASURE_APP_ID)
    {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

const CSS: &str = r#"
@define-color accent_color #0e7c66;
@define-color accent_bg_color #0e7c66;
@define-color accent_fg_color #ffffff;

.page-canvas {
  background: #c8c5bf;
}
.dark .page-canvas {
  background: #1c1c1c;
}

.thumb-canvas {
  background: transparent;
}

.placeholder-banner {
  padding: 6px 12px;
  background: alpha(@accent_bg_color, 0.15);
  color: @window_fg_color;
  font-size: 0.9em;
}
"#;

pub(crate) fn show_toast(overlay: &adw::ToastOverlay, msg: &str) {
    let t = adw::Toast::new(msg);
    t.set_timeout(3);
    overlay.add_toast(t);
}
