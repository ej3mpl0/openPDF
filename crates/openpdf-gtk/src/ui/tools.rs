// SPDX-License-Identifier: MPL-2.0
//! libadwaita dialogs for core organize / optimize / protect / convert tools.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::{
    apply_overlay, crop_pages, extract, images_to_pdf, insert_pages, merge_encryption_source,
    optimize, parse_page_ranges, pdf_to_rasters, protect, raster_to_png, sanitize_metadata, tr,
    tr_args, unprotect, CropBox, Document, EncryptionSourceKind, Engine, Error, OptimizeOptions,
    Overlay, OverlayKind, PdfPermissions, ProtectOptions, SignedOp,
};

use crate::i18n::{format_core_error, t};
use crate::session::Opened;
use crate::Lang;
use zeroize::Zeroizing;

use super::password;
use super::show_toast;
use super::viewer::DocumentView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Insert,
    Extract,
    Crop,
    Optimize,
    Protect,
    Unprotect,
    Sanitize,
    Watermark,
    PageNumbers,
    FromImages,
    ToImages,
}

impl Tool {
    pub fn title(self, lang: Lang) -> &'static str {
        t(lang, self.title_msgid())
    }

    fn title_msgid(self) -> &'static str {
        match self {
            Self::Insert => "Insert pages from another PDF",
            Self::Extract => "Extract pages to a new PDF",
            Self::Crop => "Set a crop box on pages",
            Self::Optimize => "Recompress images, drop unused objects, linearize",
            Self::Protect => "Encrypt with a user/owner password (AES-256)",
            Self::Unprotect => "Remove password protection",
            Self::Sanitize => "Remove metadata, JavaScript, and embedded-file names",
            Self::Watermark => "Stamp a watermark",
            Self::PageNumbers => "Add page numbers",
            Self::FromImages => "Build a PDF from images",
            Self::ToImages => "Render pages to PNG files",
        }
    }

    /// `Some` when this tool rewrites the open file and must refuse signatures.
    pub fn signed_block(self) -> Option<SignedOp> {
        match self {
            Self::Insert | Self::Crop | Self::Sanitize | Self::Watermark | Self::PageNumbers => {
                Some(SignedOp::Edit)
            }
            Self::Optimize | Self::Protect | Self::Unprotect => Some(SignedOp::Rewrite),
            Self::Extract | Self::FromImages | Self::ToImages => None,
        }
    }

    fn needs_document(self) -> bool {
        !matches!(self, Self::FromImages)
    }
}

pub fn present(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
    engine: Option<Engine>,
    tool: Tool,
) {
    if tool.needs_document() && view.is_none() {
        show_toast(overlay, t(lang, "Open a PDF before using this tool."));
        return;
    }
    if let Some(view) = &view {
        if view.opened().is_placeholder() && tool.needs_document() {
            if !matches!(tool, Tool::Extract | Tool::ToImages) {
                show_toast(overlay, &tr("Page edits need a real document."));
                return;
            }
        }
    }
    match tool {
        Tool::Insert => insert_dialog(parent, overlay, lang, view),
        Tool::Extract => extract_dialog(parent, overlay, lang, view),
        Tool::Crop => crop_dialog(parent, overlay, lang, view),
        Tool::Optimize => optimize_dialog(parent, overlay, lang, view),
        Tool::Protect => protect_dialog(parent, overlay, lang, view),
        Tool::Unprotect => unprotect_dialog(parent, overlay, lang, view),
        Tool::Sanitize => sanitize_dialog(parent, overlay, lang, view),
        Tool::Watermark => overlay_dialog(parent, overlay, lang, view, OverlayKind::Watermark),
        Tool::PageNumbers => overlay_dialog(parent, overlay, lang, view, OverlayKind::PageNumber),
        Tool::FromImages => from_images_dialog(parent, overlay, lang, engine),
        Tool::ToImages => to_images_dialog(parent, overlay, lang, view, engine),
    }
}

fn page_count(view: &DocumentView) -> u32 {
    view.opened().page_count() as u32
}

fn signed_banner(lang: Lang, tool: Tool, opened: &Opened) -> Option<gtk::Widget> {
    let op = tool.signed_block()?;
    if !opened.is_signed() {
        return None;
    }
    let banner = adw::Banner::new(&tr_args_signed(lang, op));
    banner.set_revealed(true);
    Some(banner.upcast())
}

fn tr_args_signed(_lang: Lang, op: SignedOp) -> String {
    openpdf_core::tr_args(
        "This document is signed; {op} is disabled.",
        &[("op", &openpdf_core::tr(op.msgid()))],
    )
}

fn apply_in_place(
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Rc<DocumentView>,
    f: impl FnOnce(&Document) -> openpdf_core::Result<Document>,
) {
    let Some(engine) = view.opened().engine() else {
        show_toast(overlay, t(lang, "Page edits need a real document."));
        return;
    };
    let prepared = match view.opened() {
        crate::session::Opened::Core(session) => session.apply_tool_prepare(f),
        crate::session::Opened::Mock(_) => {
            show_toast(overlay, t(lang, "Page edits need a real document."));
            return;
        }
    };
    let (old_id, new) = match prepared {
        Ok(v) => v,
        Err(e) => {
            show_toast(overlay, &e.to_string());
            return;
        }
    };
    let parent = overlay
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok());
    let busy = parent
        .as_ref()
        .map(|w| crate::bind::present_busy(w, lang, t(lang, "Document updated.")));
    let overlay = overlay.clone();
    crate::bind::spawn_engine_bind(
        move || {
            engine
                .bind(&new)
                .map(|_| new)
                .map_err(|e| crate::i18n::format_core_error(&e))
        },
        move |result| {
            if let Some(b) = busy {
                b.close();
            }
            match result {
                Ok(new) => {
                    view.opened().adopt_document(old_id, new);
                    if let Err(e) = view.opened().save() {
                        show_toast(&overlay, &e.to_string());
                    } else {
                        show_toast(&overlay, t(lang, "Document updated."));
                    }
                    view.ensure_laid_out();
                    view.refresh_lock_chrome();
                    view.queue_repaint();
                }
                Err(e) => show_toast(&overlay, &e),
            }
        },
    );
}

struct Form {
    win: adw::Window,
    col: gtk::Box,
    apply: gtk::Button,
    blocked: bool,
}

fn form(parent: &impl IsA<gtk::Window>, lang: Lang, tool: Tool, opened: Option<&Opened>) -> Form {
    let win = adw::Window::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_default_size(420, 320);
    win.set_title(Some(tool.title(lang)));

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
    col.set_margin_top(16);
    col.set_margin_bottom(16);
    col.set_margin_start(16);
    col.set_margin_end(16);

    let mut blocked = false;
    if let Some(opened) = opened {
        if let Some(banner) = signed_banner(lang, tool, opened) {
            toolbar.add_top_bar(&banner);
            blocked = true;
        }
    }

    let apply = gtk::Button::with_label(t(lang, "Apply"));
    apply.add_css_class("suggested-action");
    apply.set_sensitive(!blocked);
    let cancel = gtk::Button::with_label(t(lang, "Cancel"));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&cancel);
    buttons.append(&apply);
    let win_c = win.clone();
    cancel.connect_clicked(move |_| win_c.close());

    toolbar.set_content(Some(&col));
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.append(&toolbar);
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    footer.set_margin_start(16);
    footer.set_margin_end(16);
    footer.set_margin_bottom(16);
    footer.append(&buttons);
    outer.append(&footer);
    win.set_content(Some(&outer));

    Form {
        win,
        col,
        apply,
        blocked,
    }
}

fn entry(placeholder: &str) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_placeholder_text(Some(placeholder));
    e.set_hexpand(true);
    e
}

fn labeled(lang: Lang, msgid: &'static str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let l = gtk::Label::new(Some(t(lang, msgid)));
    l.set_xalign(0.0);
    l.add_css_class("dim-label");
    box_.append(&l);
    box_.append(child);
    box_
}

fn password_entry() -> gtk::PasswordEntry {
    // Never log or toast the contents. Errors go through InvalidKind only.
    let e = gtk::PasswordEntry::new();
    e.set_show_peek_icon(true);
    e.set_hexpand(true);
    e.set_activates_default(true);
    e
}

fn doc_file_name(doc: &Document, fallback: &str) -> String {
    doc.path()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// Localized merge/extract warning from core's public
/// [`merge_encryption_source`]. Unknown future kinds use a generic sentence.
/// When more than one input needs a user password, a second sentence says the
/// other files' passwords will not open the result.
fn merge_encryption_warning(docs: &[Document], names: &[&str]) -> Option<String> {
    let (i, kind) = merge_encryption_source(docs)?;
    let file = names
        .get(i)
        .copied()
        .filter(|s| !s.is_empty())
        .unwrap_or("?");
    let file_args = &[("file", file)];
    #[allow(unreachable_patterns)]
    let mut msg = match kind {
        EncryptionSourceKind::UserPassword => tr_args(
            "The result will be protected with the password of «{file}»",
            file_args,
        ),
        EncryptionSourceKind::OwnerOnly => tr_args(
            "The result will open without a password but keep the restrictions of «{file}»",
            file_args,
        ),
        _ => tr_args("The result will keep the protection of «{file}»", file_args),
    };
    let user_passwords = docs
        .iter()
        .filter(|d| {
            matches!(
                merge_encryption_source(std::slice::from_ref(*d)),
                Some((_, EncryptionSourceKind::UserPassword))
            )
        })
        .count();
    if user_passwords > 1 {
        msg.push(' ');
        msg.push_str(&tr(
            "The passwords of the other files will no longer open it.",
        ));
    }
    Some(msg)
}

fn set_merge_warning(warn: &gtk::Label, docs: &[Document], names: &[&str]) {
    match merge_encryption_warning(docs, names) {
        Some(msg) => {
            warn.set_text(&msg);
            warn.set_visible(true);
        }
        None => {
            warn.set_text("");
            warn.set_visible(false);
        }
    }
}

fn ranges_of(spec: &str, count: u32) -> openpdf_core::Result<Vec<openpdf_core::PageRange>> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(Vec::new());
    }
    parse_page_ranges(spec, count)
}

#[derive(Clone)]
struct InsertSourceUi {
    view: Rc<DocumentView>,
    source: Rc<RefCell<Option<(PathBuf, Document)>>>,
    source_l: gtk::Label,
    warn: gtk::Label,
}

fn adopt_insert_source(
    parent: &impl IsA<gtk::Window>,
    lang: Lang,
    overlay: &adw::ToastOverlay,
    ui: &InsertSourceUi,
    path: PathBuf,
) {
    match Document::open(&path) {
        Ok(doc) => store_insert_source(ui, path, doc),
        Err(Error::PasswordRequired) => {
            let ui = InsertSourceUi {
                view: ui.view.clone(),
                source: ui.source.clone(),
                source_l: ui.source_l.clone(),
                warn: ui.warn.clone(),
            };
            let dest = path.clone();
            password::prompt(parent, lang, &path, move |pw, done| {
                done(Document::open_with_password(&dest, pw).map(|doc| {
                    store_insert_source(&ui, dest.clone(), doc);
                }));
            });
        }
        Err(e) => show_toast(overlay, &format_core_error(&e)),
    }
}

fn store_insert_source(ui: &InsertSourceUi, path: PathBuf, src: Document) {
    ui.source_l.set_text(&path.display().to_string());
    let src_name = doc_file_name(&src, "PDF");
    let _ = ui.view.opened().with_document(|cur| {
        let cur_name = doc_file_name(cur, "PDF");
        set_merge_warning(
            &ui.warn,
            &[cur.clone(), src.clone()],
            &[&cur_name, &src_name],
        );
        Ok(())
    });
    *ui.source.borrow_mut() = Some((path, src));
}

fn insert_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Insert, Some(view.opened()));
    let source = Rc::new(RefCell::new(None::<(PathBuf, Document)>));
    let pick = gtk::Button::with_label(t(lang, "Source PDF"));
    let source_l = gtk::Label::new(Some(t(lang, "Source PDF")));
    source_l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    source_l.add_css_class("dim-label");
    source_l.set_xalign(0.0);
    let warn = gtk::Label::new(None);
    warn.add_css_class("warning");
    warn.set_wrap(true);
    warn.set_xalign(0.0);
    warn.set_visible(false);
    let at = gtk::SpinButton::with_range(1.0, f64::from(page_count(&view).max(1) + 1), 1.0);
    at.set_value(1.0);
    let pages = entry(t(lang, "1-based ranges, e.g. 1-3,5"));
    form.col.append(&labeled(lang, "Source PDF", &pick));
    form.col.append(&source_l);
    form.col
        .append(&labeled(lang, "1-based insert position", &at));
    form.col
        .append(&labeled(lang, "1-based ranges, e.g. 1-3,5", &pages));
    form.col.append(&warn);

    pick.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let ui = InsertSourceUi {
            view: view.clone(),
            source: source.clone(),
            source_l: source_l.clone(),
            warn: warn.clone(),
        };
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title(t(lang, "Source PDF"))
                .modal(true)
                .build();
            let filter = gtk::FileFilter::new();
            filter.add_suffix("pdf");
            dialog.set_default_filter(Some(&filter));
            let ui = InsertSourceUi {
                view: ui.view.clone(),
                source: ui.source.clone(),
                source_l: ui.source_l.clone(),
                warn: ui.warn.clone(),
            };
            let overlay = overlay.clone();
            let win = win.clone();
            glib::spawn_future_local(async move {
                if let Ok(file) = dialog.open_future(Some(&win)).await {
                    if let Some(path) = file.path() {
                        adopt_insert_source(&win, lang, &overlay, &ui, path);
                    }
                }
            });
        }
    });

    form.apply.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let view = view.clone();
        let blocked = form.blocked;
        move |_| {
            if blocked {
                return;
            }
            let Some((_path, src)) = source.borrow().clone() else {
                show_toast(&overlay, t(lang, "Choose a source PDF."));
                return;
            };
            let count = src.page_count();
            let ranges = match ranges_of(&pages.text(), count) {
                Ok(r) if !r.is_empty() => r,
                Ok(_) => vec![openpdf_core::PageRange::new(0, count.saturating_sub(1))
                    .unwrap_or_else(|_| openpdf_core::PageRange::single(0))],
                Err(e) => {
                    show_toast(&overlay, &format_core_error(&e));
                    return;
                }
            };
            let at_idx = (at.value() as u32).saturating_sub(1);
            apply_in_place(&overlay, lang, view.clone(), |doc| {
                insert_pages(doc, at_idx, &src, &ranges)
            });
            win.close();
        }
    });
    form.win.present();
}

fn extract_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Extract, Some(view.opened()));
    let pages = entry(t(lang, "1-based ranges, e.g. 1-3,5"));
    form.col
        .append(&labeled(lang, "1-based ranges, e.g. 1-3,5", &pages));
    let warn = gtk::Label::new(None);
    warn.add_css_class("warning");
    warn.set_wrap(true);
    warn.set_xalign(0.0);
    warn.set_visible(false);
    let _ = view.opened().with_document(|doc| {
        let name = doc_file_name(doc, "PDF");
        set_merge_warning(&warn, std::slice::from_ref(doc), &[&name]);
        Ok(())
    });
    form.col.append(&warn);
    form.apply.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let view = view.clone();
        let parent = parent.upcast_ref::<gtk::Window>().clone();
        move |_| {
            let count = page_count(&view);
            let ranges = match ranges_of(&pages.text(), count) {
                Ok(r) if !r.is_empty() => r,
                Ok(_) => {
                    show_toast(&overlay, &tr("no page ranges"));
                    return;
                }
                Err(e) => {
                    show_toast(&overlay, &format_core_error(&e));
                    return;
                }
            };
            let extracted = match view.opened().with_document(|doc| extract(doc, &ranges)) {
                Ok(d) => d,
                Err(e) => {
                    show_toast(&overlay, &e.to_string());
                    return;
                }
            };
            save_pdf_dialog(&parent, lang, &overlay, extracted);
            win.close();
        }
    });
    form.win.present();
}

fn crop_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Crop, Some(view.opened()));
    let box_e = entry(t(lang, "Crop box llx,lly,urx,ury in points"));
    box_e.set_text("0,0,612,792");
    let pages = entry(t(lang, "1-based ranges, e.g. 1-3,5"));
    form.col
        .append(&labeled(lang, "Crop box llx,lly,urx,ury in points", &box_e));
    form.col
        .append(&labeled(lang, "1-based ranges, e.g. 1-3,5", &pages));
    form.apply.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let view = view.clone();
        let blocked = form.blocked;
        move |_| {
            if blocked {
                return;
            }
            let crop = match parse_crop(&box_e.text()) {
                Ok(c) => c,
                Err(e) => {
                    show_toast(&overlay, &format_core_error(&e));
                    return;
                }
            };
            let ranges = match ranges_of(&pages.text(), page_count(&view)) {
                Ok(r) => r,
                Err(e) => {
                    show_toast(&overlay, &format_core_error(&e));
                    return;
                }
            };
            apply_in_place(&overlay, lang, view.clone(), |doc| {
                crop_pages(doc, &ranges, crop)
            });
            win.close();
        }
    });
    form.win.present();
}

fn parse_crop(spec: &str) -> openpdf_core::Result<CropBox> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(openpdf_core::Error::invalid(
            openpdf_core::InvalidKind::CropInverted,
        ));
    }
    let nums: Result<Vec<f32>, _> = parts.iter().map(|p| p.parse()).collect();
    let nums =
        nums.map_err(|_| openpdf_core::Error::invalid(openpdf_core::InvalidKind::CropInverted))?;
    CropBox::new(nums[0], nums[1], nums[2], nums[3])
}

fn optimize_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Optimize, Some(view.opened()));
    let quality = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
    quality.set_value(75.0);
    let linear = gtk::CheckButton::with_label(t(lang, "Skip the linearized hint"));
    form.col
        .append(&labeled(lang, "JPEG quality 1-100", &quality));
    form.col.append(&linear);
    form.apply.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let view = view.clone();
        let blocked = form.blocked;
        move |_| {
            if blocked {
                return;
            }
            let opts = OptimizeOptions {
                jpeg_quality: Some(quality.value() as u8),
                drop_unused: true,
                linearize: !linear.is_active(),
            };
            apply_in_place(&overlay, lang, view.clone(), |doc| optimize(doc, &opts));
            win.close();
        }
    });
    form.win.present();
}

fn protect_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Protect, Some(view.opened()));
    let user = password_entry();
    let owner = password_entry();
    form.col.append(&labeled(lang, "User password", &user));
    form.col
        .append(&labeled(lang, "Owner password (defaults to user)", &owner));
    form.apply.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let view = view.clone();
        let blocked = form.blocked;
        move |_| {
            if blocked {
                return;
            }
            let user_pw = Zeroizing::new(user.text().to_string());
            let owner_pw = Zeroizing::new(owner.text().to_string());
            user.set_text("");
            owner.set_text("");
            let opts = match ProtectOptions::new(user_pw.as_str(), owner_pw.as_str()) {
                Ok(o) => {
                    let mut o = o;
                    o.permissions = PdfPermissions::all();
                    o
                }
                Err(e) => {
                    show_toast(&overlay, &format_core_error(&e));
                    return;
                }
            };
            apply_in_place(&overlay, lang, view.clone(), |doc| protect(doc, &opts));
            win.close();
        }
    });
    form.win.present();
}

fn unprotect_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Unprotect, Some(view.opened()));
    let password = password_entry();
    form.col
        .append(&labeled(lang, "User or owner password", &password));
    form.apply.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let view = view.clone();
        let blocked = form.blocked;
        move |_| {
            if blocked {
                return;
            }
            let pw = Zeroizing::new(password.text().to_string());
            password.set_text("");
            apply_in_place(&overlay, lang, view.clone(), |doc| {
                unprotect(doc, pw.as_str())
            });
            win.close();
        }
    });
    form.win.present();
}

fn sanitize_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Sanitize, Some(view.opened()));
    let note = gtk::Label::new(Some(t(
        lang,
        "Remove metadata, JavaScript, and embedded-file names",
    )));
    note.set_wrap(true);
    note.set_xalign(0.0);
    form.col.append(&note);
    form.apply.connect_clicked({
        let win = form.win.clone();
        let overlay = overlay.clone();
        let view = view.clone();
        let blocked = form.blocked;
        move |_| {
            if blocked {
                return;
            }
            apply_in_place(&overlay, lang, view.clone(), sanitize_metadata);
            win.close();
        }
    });
    form.win.present();
}

fn overlay_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
    kind: OverlayKind,
) {
    let Some(view) = view else { return };
    let tool = match kind {
        OverlayKind::PageNumber => Tool::PageNumbers,
        _ => Tool::Watermark,
    };
    let form = form(parent, lang, tool, Some(view.opened()));
    let text = entry(match kind {
        OverlayKind::PageNumber => t(lang, "Number format; {page} and {total} are placeholders"),
        _ => t(lang, "Overlay text"),
    });
    if kind == OverlayKind::PageNumber {
        text.set_text("{page} / {total}");
    }
    let pages = entry(t(lang, "1-based ranges, e.g. 1-3,5"));
    form.col.append(&labeled(
        lang,
        if kind == OverlayKind::PageNumber {
            "Number format; {page} and {total} are placeholders"
        } else {
            "Overlay text"
        },
        &text,
    ));
    form.col
        .append(&labeled(lang, "1-based ranges, e.g. 1-3,5", &pages));
    form.apply.connect_clicked({
        let win = form.win.clone();
        let toast = overlay.clone();
        let view = view.clone();
        let blocked = form.blocked;
        move |_| {
            if blocked {
                return;
            }
            let mut stamp = match Overlay::new(kind, text.text().as_str()) {
                Ok(o) => o,
                Err(e) => {
                    show_toast(&toast, &format_core_error(&e));
                    return;
                }
            };
            match ranges_of(&pages.text(), page_count(&view)) {
                Ok(r) => stamp.pages = r,
                Err(e) => {
                    show_toast(&toast, &format_core_error(&e));
                    return;
                }
            }
            apply_in_place(&toast, lang, view.clone(), |doc| apply_overlay(doc, &stamp));
            win.close();
        }
    });
    form.win.present();
}

fn from_images_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    _engine: Option<Engine>,
) {
    let dialog = gtk::FileDialog::builder()
        .title(t(lang, "Image files"))
        .modal(true)
        .build();
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(t(lang, "Image files")));
    filter.add_mime_type("image/png");
    filter.add_mime_type("image/jpeg");
    filter.add_suffix("png");
    filter.add_suffix("jpg");
    filter.add_suffix("jpeg");
    dialog.set_default_filter(Some(&filter));
    let parent = parent.upcast_ref::<gtk::Window>().clone();
    let overlay = overlay.clone();
    glib::spawn_future_local(async move {
        let files = match dialog.open_multiple_future(Some(&parent)).await {
            Ok(list) => list,
            Err(_) => return,
        };
        let mut paths = Vec::new();
        for i in 0..files.n_items() {
            if let Some(obj) = files.item(i) {
                if let Ok(file) = obj.downcast::<gio::File>() {
                    if let Some(path) = file.path() {
                        paths.push(path);
                    }
                }
            }
        }
        if paths.is_empty() {
            show_toast(&overlay, &tr("no input images"));
            return;
        }
        match images_to_pdf(&paths) {
            Ok(doc) => save_pdf_dialog(&parent, lang, &overlay, doc),
            Err(e) => show_toast(&overlay, &format_core_error(&e)),
        }
    });
}

fn to_images_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
    engine: Option<Engine>,
) {
    let Some(view) = view else { return };
    let Some(engine) = engine else {
        show_toast(
            overlay,
            t(
                lang,
                "Preview uses a placeholder renderer because PDFium is not available.",
            ),
        );
        return;
    };
    let dialog = gtk::FileDialog::builder()
        .title(t(lang, "Output path"))
        .modal(true)
        .build();
    let parent = parent.upcast_ref::<gtk::Window>().clone();
    let overlay = overlay.clone();
    glib::spawn_future_local(async move {
        let folder = match dialog.select_folder_future(Some(&parent)).await {
            Ok(f) => f,
            Err(_) => return,
        };
        let Some(dir) = folder.path() else { return };
        let bytes = match view.opened().to_bytes() {
            Ok(b) => b,
            Err(e) => {
                show_toast(&overlay, &e.to_string());
                return;
            }
        };
        let doc = match Document::from_bytes(bytes, None) {
            Ok(d) => d,
            Err(e) => {
                show_toast(&overlay, &format_core_error(&e));
                return;
            }
        };
        let busy = crate::bind::present_busy(&parent, lang, t(lang, "Document updated."));
        crate::bind::spawn_engine_bind(
            move || pdf_to_rasters(&engine, &doc, 1.0).map_err(|e| format_core_error(&e)),
            move |result| {
                busy.close();
                match result {
                    Ok(rasters) => {
                        for (i, raster) in rasters.iter().enumerate() {
                            match raster_to_png(raster) {
                                Ok(png) => {
                                    let path = dir.join(format!("page-{}.png", i + 1));
                                    if let Err(e) = std::fs::write(&path, png) {
                                        show_toast(&overlay, &e.to_string());
                                        return;
                                    }
                                }
                                Err(e) => {
                                    show_toast(&overlay, &format_core_error(&e));
                                    return;
                                }
                            }
                        }
                        show_toast(
                            &overlay,
                            &openpdf_core::tr_args(
                                "wrote {path}",
                                &[("path", &dir.display().to_string())],
                            ),
                        );
                    }
                    Err(e) => show_toast(&overlay, &e),
                }
            },
        );
    });
}

fn save_pdf_dialog(
    parent: &impl IsA<gtk::Window>,
    lang: Lang,
    overlay: &adw::ToastOverlay,
    doc: Document,
) {
    let dialog = gtk::FileDialog::builder()
        .title(t(lang, "Output path"))
        .modal(true)
        .initial_name("document.pdf")
        .build();
    let parent = parent.upcast_ref::<gtk::Window>().clone();
    let overlay = overlay.clone();
    glib::spawn_future_local(async move {
        let file = match dialog.save_future(Some(&parent)).await {
            Ok(f) => f,
            Err(_) => return,
        };
        let Some(path) = file.path() else { return };
        match write_doc(&doc, &path) {
            Ok(()) => show_toast(
                &overlay,
                &openpdf_core::tr_args("wrote {path}", &[("path", &path.display().to_string())]),
            ),
            Err(e) => show_toast(&overlay, &e),
        }
    });
}

fn write_doc(doc: &Document, path: &Path) -> Result<(), String> {
    let bytes = doc.to_bytes().map_err(|e| format_core_error(&e))?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_docs_block_rewrites_not_extract() {
        assert_eq!(Tool::Insert.signed_block(), Some(SignedOp::Edit));
        assert_eq!(Tool::Optimize.signed_block(), Some(SignedOp::Rewrite));
        assert_eq!(Tool::Protect.signed_block(), Some(SignedOp::Rewrite));
        assert_eq!(Tool::Watermark.signed_block(), Some(SignedOp::Edit));
        assert_eq!(Tool::Extract.signed_block(), None);
        assert_eq!(Tool::FromImages.signed_block(), None);
        assert_eq!(Tool::ToImages.signed_block(), None);
    }

    #[test]
    fn password_errors_do_not_echo_secrets() {
        let secret = "s3cret-pass-value";
        let err = openpdf_core::Error::invalid(openpdf_core::InvalidKind::WrongPassword);
        let msg = format_core_error(&err);
        assert!(!msg.contains(secret), "{msg}");
        assert_eq!(msg, "wrong password");
        let required = format_core_error(&openpdf_core::Error::invalid(
            openpdf_core::InvalidKind::PasswordRequired,
        ));
        assert_eq!(required, "password is required");
        assert!(!required.contains(secret));
    }

    fn corpus_pdf(name: &str) -> Option<Document> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .join(name);
        Document::open(path).ok()
    }

    #[test]
    fn merge_warning_follows_core_source_kind() {
        let Some(plain) = corpus_pdf("hello.pdf") else {
            return;
        };
        let Some(other) = corpus_pdf("two_page.pdf") else {
            return;
        };
        let user = protect(&plain, &ProtectOptions::new("user-a", "owner-a").unwrap()).unwrap();
        let owner = protect(&plain, &ProtectOptions::new("", "owner-only").unwrap()).unwrap();
        let _g = openpdf_core::test_support::locale_test_guard();
        openpdf_core::set_locale("en");
        assert_eq!(
            merge_encryption_source(&[plain.clone(), user.clone()]),
            Some((1, EncryptionSourceKind::UserPassword))
        );
        assert_eq!(
            merge_encryption_warning(&[plain.clone(), user.clone()], &["plain.pdf", "a.pdf"])
                .as_deref(),
            Some("The result will be protected with the password of «a.pdf»")
        );
        assert_eq!(
            merge_encryption_warning(&[plain.clone(), owner.clone()], &["plain.pdf", "o.pdf"])
                .as_deref(),
            Some("The result will open without a password but keep the restrictions of «o.pdf»")
        );
        let user_b = protect(&other, &ProtectOptions::new("user-b", "owner-b").unwrap()).unwrap();
        let two =
            merge_encryption_warning(&[user.clone(), user_b.clone()], &["a.pdf", "b.pdf"]).unwrap();
        assert_eq!(
            two,
            "The result will be protected with the password of «a.pdf» The passwords of the other files will no longer open it."
        );
        assert!(merge_encryption_warning(&[plain.clone(), other.clone()], &["x", "y"]).is_none());

        openpdf_core::set_locale("es");
        let es = merge_encryption_warning(&[user, user_b], &["a.pdf", "b.pdf"]).unwrap();
        let expected = format!(
            "{} {}",
            tr_args(
                "The result will be protected with the password of «{file}»",
                &[("file", "a.pdf")],
            ),
            tr("The passwords of the other files will no longer open it."),
        );
        assert_eq!(es, expected);
        assert_ne!(
            es,
            "The result will be protected with the password of «a.pdf» The passwords of the other files will no longer open it."
        );
        assert_eq!(
            merge_encryption_warning(&[owner], &["o.pdf"]).unwrap(),
            tr_args(
                "The result will open without a password but keep the restrictions of «{file}»",
                &[("file", "o.pdf")],
            )
        );
        openpdf_core::set_locale("en");
    }

    #[test]
    fn protect_then_merge_without_saving_keeps_user_password() {
        let Some(plain) = corpus_pdf("hello.pdf") else {
            return;
        };
        let Some(other) = corpus_pdf("two_page.pdf") else {
            return;
        };
        let protected = protect(
            &plain,
            &ProtectOptions::new("mem-secret", "mem-owner").unwrap(),
        )
        .unwrap();
        assert!(
            protected.path().is_none(),
            "protect must stay in memory; saving would hide a merge-source bug"
        );
        assert_eq!(
            merge_encryption_source(&[protected.clone(), other.clone()]),
            Some((0, EncryptionSourceKind::UserPassword))
        );
        let _g = openpdf_core::test_support::locale_test_guard();
        openpdf_core::set_locale("en");
        assert_eq!(
            merge_encryption_warning(
                &[protected.clone(), other.clone()],
                &["mem.pdf", "hello.pdf"]
            )
            .as_deref(),
            Some("The result will be protected with the password of «mem.pdf»")
        );
        let merged = openpdf_core::merge(&[protected, other]).unwrap();
        let bytes = merged.to_bytes().unwrap();
        assert!(
            matches!(
                Document::from_bytes(bytes, None),
                Err(Error::PasswordRequired)
            ),
            "unsaved protect() must still be the merge encryption source"
        );
    }
}
