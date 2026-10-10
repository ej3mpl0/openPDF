// SPDX-License-Identifier: MPL-2.0
use std::cell::{Cell, RefCell};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::backend::MockDocument;
use crate::i18n::{format_core_error, t};
use crate::session::{sample_pdf_path, MockView, Opened, Session};
use crate::settings::{self, Settings};
use crate::sign::format_verify_results;
use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{Engine, Signer, Verifier};

use super::about;
use super::home::{self, HomeAction};
use super::preferences;
use super::show_toast;
use super::sign as sign_ui;
use super::tools::{self, Tool};
use super::viewer::{self, DocumentView};

#[derive(Clone)]
struct Ctx {
    /// Created on the first document open, not at window construction, so the
    /// first frame is not blocked on PDFium.
    engine: Rc<RefCell<Option<Engine>>>,
    signer: Arc<dyn Signer>,
    verifier: Arc<dyn Verifier>,
    window: adw::ApplicationWindow,
    tab_view: adw::TabView,
    toast: adw::ToastOverlay,
    settings: Rc<RefCell<Settings>>,
    current: Rc<RefCell<Option<Rc<DocumentView>>>>,
    home_slot: gtk::Box,
    home_filled: Rc<Cell<bool>>,
}

#[derive(Clone)]
pub struct OpenPdfWindow {
    window: adw::ApplicationWindow,
    ctx: Ctx,
}

impl OpenPdfWindow {
    pub fn new(
        app: &adw::Application,
        settings: Settings,
        signer: Arc<dyn Signer>,
        verifier: Arc<dyn Verifier>,
    ) -> Self {
        let lang = settings.lang();
        let settings = Rc::new(RefCell::new(settings));
        let current = Rc::new(RefCell::new(None));
        let engine = Rc::new(RefCell::new(None));

        let window = adw::ApplicationWindow::new(app);
        window.set_title(Some(t(lang, "openPDF")));
        window.set_default_size(1180, 800);

        let toast = adw::ToastOverlay::new();
        let tab_view = adw::TabView::new();
        tab_view.set_vexpand(true);

        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(true);

        let header = adw::HeaderBar::new();
        let open_btn = gtk::Button::from_icon_name("document-open-symbolic");
        open_btn.set_tooltip_text(Some(t(lang, "Open…")));
        header.pack_start(&open_btn);

        let sidebar_btn = gtk::ToggleButton::new();
        sidebar_btn.set_icon_name("sidebar-show-symbolic");
        sidebar_btn.set_tooltip_text(Some(t(lang, "Thumbnails")));
        sidebar_btn.set_active(settings.borrow().show_thumbnails);
        header.pack_start(&sidebar_btn);

        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.set_menu_model(Some(&app_menu(lang)));
        header.pack_end(&menu_btn);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        toast.set_child(Some(&tab_view));
        toolbar.set_content(Some(&toast));
        window.set_content(Some(&toolbar));

        let home_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        home_slot.set_vexpand(true);
        let home_filled = Rc::new(Cell::new(false));

        let ctx = Ctx {
            engine,
            signer,
            verifier,
            window: window.clone(),
            tab_view: tab_view.clone(),
            toast: toast.clone(),
            settings: settings.clone(),
            current: current.clone(),
            home_slot: home_slot.clone(),
            home_filled: home_filled.clone(),
        };

        install_actions(app, &ctx);

        let open_cb = {
            let ctx = ctx.clone();
            move || open_file_dialog(&ctx)
        };
        open_btn.connect_clicked({
            let open_cb = open_cb.clone();
            move |_| open_cb()
        });

        sidebar_btn.connect_toggled({
            let ctx = ctx.clone();
            move |btn| {
                ctx.settings.borrow_mut().show_thumbnails = btn.is_active();
                let _ = settings::save(&ctx.settings.borrow());
                if let Some(view) = ctx.current.borrow().as_ref() {
                    view.set_sidebar(btn.is_active());
                }
            }
        });

        // Chrome only until after present(): recent-files I/O and the tools
        // list stay off the first frame. `--measure` never fills this tab.
        let home_page = tab_view.append(&home_slot);
        home_page.set_title(t(lang, "openPDF"));
        tab_view.set_page_pinned(&home_page, true);

        tab_view.connect_selected_page_notify({
            let current = current.clone();
            let window = window.clone();
            move |tv| {
                if let Some(page) = tv.selected_page() {
                    window.set_title(Some(&page.title()));
                    if page.is_pinned() {
                        *current.borrow_mut() = None;
                    }
                }
            }
        });

        tab_view.connect_close_page(|view, page| {
            if page.is_pinned() {
                view.close_page_finish(page, false);
            } else {
                view.close_page_finish(page, true);
            }
            true
        });

        Self { window, ctx }
    }

    /// Apply on-disk settings after `present()` so startup does not wait on
    /// `~/.config` or recent-files reads. Restores the last document if asked.
    pub fn hydrate_settings(&self) {
        let loaded = settings::load();
        loaded.apply_locale();
        super::apply_theme(loaded.theme);
        let restore = loaded.restore_last;
        let last = loaded.recent.first().cloned();
        *self.ctx.settings.borrow_mut() = loaded;
        if restore {
            if let Some(last) = last {
                let ctx = self.ctx.clone();
                glib::idle_add_local_once(move || {
                    open_path_inner(&ctx, Path::new(&last));
                });
            }
        }
    }

    /// Build the home / recent / tools widgets on first idle (or first visit).
    pub fn ensure_home(&self) {
        if self.ctx.home_filled.replace(true) {
            return;
        }
        let ctx = self.ctx.clone();
        let home = home::build(
            ctx.settings.clone(),
            {
                let ctx = ctx.clone();
                move || open_file_dialog(&ctx)
            },
            {
                let ctx = ctx.clone();
                move || open_sample(&ctx)
            },
            {
                let ctx = ctx.clone();
                move |path| open_path_inner(&ctx, &path)
            },
            {
                let ctx = ctx.clone();
                move |action| match action {
                    HomeAction::ViewSample => open_sample(&ctx),
                    HomeAction::Sign => start_sign(&ctx),
                    HomeAction::Comment => show_toast(
                        &ctx.toast,
                        t(ctx.settings.borrow().lang(), "Waiting on openpdf-core"),
                    ),
                    HomeAction::Tool(tool) => start_tool(&ctx, tool),
                }
            },
        );
        self.ctx.home_slot.append(&home.root);
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn widget(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn open_path(&self, path: &Path) {
        open_path_inner(&self.ctx, path);
    }

    pub fn current_view(&self) -> Option<Rc<DocumentView>> {
        self.ctx.current.borrow().clone()
    }
}

fn app_menu(lang: crate::Lang) -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some(t(lang, "Open…")), Some("app.open"));
    menu.append(Some(t(lang, "Sign with certificate…")), Some("app.sign"));
    let tools = gio::Menu::new();
    for (tool, target) in tool_menu_items() {
        if tool.is_offered() {
            tools.append(Some(tool.title(lang)), Some(target));
        }
    }
    menu.append_submenu(Some(t(lang, "Tools")), &tools);
    menu.append(Some(t(lang, "Preferences")), Some("app.preferences"));
    menu.append(Some(t(lang, "About openPDF")), Some("app.about"));
    menu.append(Some(t(lang, "Quit")), Some("app.quit"));
    menu
}

fn tool_menu_items() -> &'static [(Tool, &'static str)] {
    &[
        (Tool::Insert, "app.tool::insert"),
        (Tool::Extract, "app.tool::extract"),
        (Tool::Crop, "app.tool::crop"),
        (Tool::Optimize, "app.tool::optimize"),
        (Tool::Protect, "app.tool::protect"),
        (Tool::Unprotect, "app.tool::unprotect"),
        (Tool::Sanitize, "app.tool::sanitize"),
        (Tool::Watermark, "app.tool::watermark"),
        (Tool::PageNumbers, "app.tool::number"),
        (Tool::FromImages, "app.tool::from-images"),
        (Tool::ToImages, "app.tool::to-images"),
        (Tool::Ocr, "app.tool::ocr"),
        (Tool::Redact, "app.tool::redact"),
        (Tool::Compare, "app.tool::compare"),
    ]
}

fn tool_from_id(id: &str) -> Option<Tool> {
    Some(match id {
        "insert" => Tool::Insert,
        "extract" => Tool::Extract,
        "crop" => Tool::Crop,
        "optimize" => Tool::Optimize,
        "protect" => Tool::Protect,
        "unprotect" => Tool::Unprotect,
        "sanitize" => Tool::Sanitize,
        "watermark" => Tool::Watermark,
        "number" => Tool::PageNumbers,
        "from-images" => Tool::FromImages,
        "to-images" => Tool::ToImages,
        "ocr" => Tool::Ocr,
        "redact" => Tool::Redact,
        "compare" => Tool::Compare,
        _ => return None,
    })
}

fn install_actions(app: &adw::Application, ctx: &Ctx) {
    let open = gio::SimpleAction::new("open", None);
    open.connect_activate({
        let ctx = ctx.clone();
        move |_, _| open_file_dialog(&ctx)
    });
    app.add_action(&open);

    let sign = gio::SimpleAction::new("sign", None);
    sign.connect_activate({
        let ctx = ctx.clone();
        move |_, _| start_sign(&ctx)
    });
    app.add_action(&sign);

    let tool = gio::SimpleAction::new("tool", Some(glib::VariantTy::STRING));
    tool.connect_activate({
        let ctx = ctx.clone();
        move |_, param| {
            let Some(id) = param.and_then(|p| p.str().map(str::to_string)) else {
                return;
            };
            if let Some(tool) = tool_from_id(&id) {
                start_tool(&ctx, tool);
            }
        }
    });
    app.add_action(&tool);

    let prefs = gio::SimpleAction::new("preferences", None);
    prefs.connect_activate({
        let ctx = ctx.clone();
        move |_, _| preferences::present(&ctx.window, ctx.settings.clone())
    });
    app.add_action(&prefs);

    let about_a = gio::SimpleAction::new("about", None);
    about_a.connect_activate({
        let ctx = ctx.clone();
        move |_, _| about::present(&ctx.window, ctx.settings.borrow().lang())
    });
    app.add_action(&about_a);

    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(clone!(@weak app => move |_, _| app.quit()));
    app.add_action(&quit);

    add_win_action(&ctx.window, "zoom-in", ctx, |v| v.zoom_in());
    add_win_action(&ctx.window, "zoom-out", ctx, |v| v.zoom_out());
    add_win_action(&ctx.window, "toggle-sidebar", ctx, |v| v.toggle_sidebar());
    add_win_action(&ctx.window, "find", ctx, |v| v.reveal_find());
    add_win_action(&ctx.window, "next-page", ctx, |v| v.next_page());
    add_win_action(&ctx.window, "prev-page", ctx, |v| v.prev_page());
    add_win_action(&ctx.window, "rotate", ctx, |v| {
        let _ = v.rotate_current(90);
    });
    add_win_action(&ctx.window, "undo", ctx, |v| {
        let _ = v.undo();
    });
    add_win_action(&ctx.window, "redo", ctx, |v| {
        let _ = v.redo();
    });
}

fn add_win_action(
    window: &adw::ApplicationWindow,
    name: &str,
    ctx: &Ctx,
    f: impl Fn(&DocumentView) + 'static,
) {
    let action = gio::SimpleAction::new(name, None);
    let current = ctx.current.clone();
    action.connect_activate(move |_, _| {
        if let Some(v) = current.borrow().as_ref() {
            f(v);
        }
    });
    window.add_action(&action);
}

fn open_file_dialog(ctx: &Ctx) {
    let lang = ctx.settings.borrow().lang();
    let dialog = gtk::FileDialog::builder()
        .title(t(lang, "Open PDF"))
        .modal(true)
        .build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(t(lang, "PDF documents")));
    filter.add_mime_type("application/pdf");
    filter.add_suffix("pdf");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&filter));

    let ctx = ctx.clone();
    glib::spawn_future_local(async move {
        match dialog.open_future(Some(&ctx.window)).await {
            Ok(file) => {
                if let Some(path) = file.path() {
                    open_path_inner(&ctx, &path);
                }
            }
            Err(_) => {}
        }
    });
}

fn open_sample(ctx: &Ctx) {
    if let Some(path) = sample_pdf_path() {
        open_path_inner(ctx, &path);
        return;
    }
    add_opened(ctx, Opened::Mock(MockView::new(MockDocument::sample(16))));
}

fn ensure_engine(ctx: &Ctx) -> Option<Engine> {
    let mut slot = ctx.engine.borrow_mut();
    if slot.is_none() {
        *slot = Engine::new().ok();
    }
    slot.clone()
}

fn open_path_inner(ctx: &Ctx, path: &Path) {
    crate::measure::mark_doc_load_start();
    let opened = if let Some(engine) = ensure_engine(ctx) {
        match Session::open(&engine, path) {
            Ok(session) => Opened::Core(session),
            Err(e) => {
                show_toast(&ctx.toast, &e.to_string());
                return;
            }
        }
    } else {
        match MockDocument::open(path) {
            Ok(doc) => Opened::Mock(MockView::new(doc)),
            Err(e) => {
                show_toast(&ctx.toast, &e.to_string());
                return;
            }
        }
    };
    crate::measure::mark_doc_loaded();
    ctx.settings.borrow_mut().push_recent(path);
    let settings = ctx.settings.clone();
    glib::idle_add_local_once(move || {
        let _ = settings::save(&settings.borrow());
    });
    add_opened(ctx, opened);
}

fn add_opened(ctx: &Ctx, opened: Opened) {
    let lang = ctx.settings.borrow().lang();
    let locked = opened.is_locked();
    let placeholder = opened.is_placeholder();
    let view = viewer::build(Rc::new(opened), &ctx.settings.borrow(), lang);
    let page = ctx.tab_view.append(view.widget());
    page.set_title(&view.title());
    if let Some(path) = view.path() {
        page.set_tooltip(&path.to_string_lossy());
    }
    ctx.tab_view.set_selected_page(&page);
    *ctx.current.borrow_mut() = Some(view.clone());
    view.request_first_tile();
    if locked {
        show_toast(
            &ctx.toast,
            t(
                lang,
                "Signed document — read-only. Rotate, delete and reorder are disabled.",
            ),
        );
        maybe_show_verify(&ctx.toast, ctx.verifier.clone(), view.opened(), lang);
    } else if placeholder {
        show_toast(
            &ctx.toast,
            t(
                lang,
                "Preview uses a placeholder renderer because PDFium is not available.",
            ),
        );
    }
}

fn maybe_show_verify(
    overlay: &adw::ToastOverlay,
    verifier: Arc<dyn Verifier>,
    opened: &Opened,
    lang: crate::Lang,
) {
    let bytes = match opened.bytes_for_verify() {
        Ok(bytes) => bytes,
        Err(e) => {
            show_toast(overlay, &e.to_string());
            return;
        }
    };
    let overlay = overlay.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(verifier.verify(&bytes));
    });
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        match rx.try_recv() {
            Ok(Ok(infos)) if !infos.is_empty() => {
                show_toast(&overlay, &format_verify_results(&infos));
                glib::ControlFlow::Break
            }
            Ok(Ok(_)) => glib::ControlFlow::Break,
            Ok(Err(e)) => {
                show_toast(&overlay, &format_core_error(&e));
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                show_toast(&overlay, t(lang, "Verification failed"));
                glib::ControlFlow::Break
            }
        }
    });
}

fn start_tool(ctx: &Ctx, tool: Tool) {
    let lang = ctx.settings.borrow().lang();
    tools::present(
        &ctx.window,
        &ctx.toast,
        lang,
        ctx.current.borrow().clone(),
        ensure_engine(ctx),
        tool,
    );
}

fn start_sign(ctx: &Ctx) {
    let lang = ctx.settings.borrow().lang();
    let Some(view) = ctx.current.borrow().clone() else {
        show_toast(&ctx.toast, t(lang, "Open a PDF before signing."));
        return;
    };
    let signer = ctx.signer.clone();
    let verifier = ctx.verifier.clone();
    let overlay = ctx.toast.clone();
    let window = ctx.window.clone();
    sign_ui::present_picker(&ctx.window, &ctx.toast, lang, signer.clone(), move |req| {
        sign_on_worker(
            &window,
            &overlay,
            lang,
            view.clone(),
            signer.clone(),
            verifier.clone(),
            req,
        );
    });
}

/// `C_Sign` / `Signer::sign` (and the PIN dialog it may invoke) stay off the
/// GTK thread. The UI shows a spinner with Cancel and applies the signed
/// bytes only after the worker returns.
fn sign_on_worker(
    parent: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    lang: crate::Lang,
    view: Rc<DocumentView>,
    signer: Arc<dyn Signer>,
    verifier: Arc<dyn Verifier>,
    req: openpdf_core::SignRequest,
) {
    if view.opened().is_placeholder() {
        view.opened().lock_after_sign();
        view.refresh_lock_chrome();
        show_toast(
            overlay,
            t(
                lang,
                "Signed document — read-only. Rotate, delete and reorder are disabled.",
            ),
        );
        return;
    }
    let bytes = match view.opened().bytes_for_sign() {
        Ok(bytes) => bytes,
        Err(e) => {
            show_toast(overlay, &e.to_string());
            return;
        }
    };

    let win = adw::Window::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_default_size(360, 200);
    win.set_title(Some(t(lang, "Signing…")));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    let spinner = gtk::Spinner::new();
    spinner.start();
    let status = adw::StatusPage::new();
    status.set_icon_name(Some("dialog-password-symbolic"));
    status.set_title(t(lang, "Signing…"));
    status.set_description(Some(t(
        lang,
        "Choose a signing certificate. The token will ask for the PIN if needed.",
    )));
    status.set_child(Some(&spinner));
    toolbar.set_content(Some(&status));
    win.set_content(Some(&toolbar));

    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel = gtk::Button::with_label(t(lang, "Cancel"));
    cancel.add_css_class("flat");
    header.pack_start(&cancel);
    cancel.connect_clicked({
        let win = win.clone();
        let cancelled = cancelled.clone();
        move |_| {
            cancelled.store(true, Ordering::SeqCst);
            win.close();
        }
    });
    win.connect_close_request({
        let cancelled = cancelled.clone();
        move |_| {
            cancelled.store(true, Ordering::SeqCst);
            gtk::glib::Propagation::Proceed
        }
    });
    win.present();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| signer.sign(&bytes, &req)));
        let _ = tx.send(result);
    });

    let overlay = overlay.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        match rx.try_recv() {
            Ok(Ok(Ok(signed))) => {
                win.close();
                if cancelled.load(Ordering::SeqCst) {
                    return glib::ControlFlow::Break;
                }
                match view.opened().apply_signed_bytes(signed) {
                    Ok(()) => {
                        view.refresh_lock_chrome();
                        show_toast(
                            &overlay,
                            t(
                                lang,
                                "Signed document — read-only. Rotate, delete and reorder are disabled.",
                            ),
                        );
                        maybe_show_verify(&overlay, verifier.clone(), view.opened(), lang);
                    }
                    Err(e) => show_toast(&overlay, &e.to_string()),
                }
                glib::ControlFlow::Break
            }
            Ok(Ok(Err(e))) => {
                win.close();
                if !cancelled.load(Ordering::SeqCst) {
                    show_toast(&overlay, &format_core_error(&e));
                }
                glib::ControlFlow::Break
            }
            Ok(Err(_)) => {
                win.close();
                if !cancelled.load(Ordering::SeqCst) {
                    show_toast(&overlay, t(lang, "Signing failed"));
                }
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                win.close();
                if !cancelled.load(Ordering::SeqCst) {
                    show_toast(&overlay, t(lang, "Signing failed"));
                }
                glib::ControlFlow::Break
            }
        }
    });
}
