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
    apply_overlay, crop_pages, extract, images_to_pdf, insert_pages, optimize, parse_page_ranges,
    pdf_to_rasters, protect, raster_to_png, sanitize_metadata, tr, unprotect, CropBox, Document,
    Engine, OptimizeOptions, Overlay, OverlayKind, PdfPermissions, ProtectOptions, SignedOp,
};

use crate::i18n::{format_core_error, t};
use crate::session::Opened;
use crate::Lang;
use zeroize::Zeroizing;

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
    view: &DocumentView,
    f: impl FnOnce(&Document) -> openpdf_core::Result<Document>,
) {
    match view.opened().apply_tool(f) {
        Ok(()) => {
            if let Err(e) = view.opened().save() {
                show_toast(overlay, &e.to_string());
            } else {
                show_toast(overlay, t(lang, "Document updated."));
            }
            view.ensure_laid_out();
            view.refresh_lock_chrome();
            view.queue_repaint();
        }
        Err(e) => show_toast(overlay, &e.to_string()),
    }
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

fn ranges_of(spec: &str, count: u32) -> openpdf_core::Result<Vec<openpdf_core::PageRange>> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(Vec::new());
    }
    parse_page_ranges(spec, count)
}

fn insert_dialog(
    parent: &impl IsA<gtk::Window>,
    overlay: &adw::ToastOverlay,
    lang: Lang,
    view: Option<Rc<DocumentView>>,
) {
    let Some(view) = view else { return };
    let form = form(parent, lang, Tool::Insert, Some(view.opened()));
    let source = Rc::new(RefCell::new(None::<PathBuf>));
    let pick = gtk::Button::with_label(t(lang, "Source PDF"));
    let source_l = gtk::Label::new(Some(t(lang, "Source PDF")));
    source_l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    source_l.add_css_class("dim-label");
    source_l.set_xalign(0.0);
    let at = gtk::SpinButton::with_range(1.0, f64::from(page_count(&view).max(1) + 1), 1.0);
    at.set_value(1.0);
    let pages = entry(t(lang, "1-based ranges, e.g. 1-3,5"));
    form.col.append(&labeled(lang, "Source PDF", &pick));
    form.col.append(&source_l);
    form.col
        .append(&labeled(lang, "1-based insert position", &at));
    form.col
        .append(&labeled(lang, "1-based ranges, e.g. 1-3,5", &pages));

    pick.connect_clicked({
        let win = form.win.clone();
        let source = source.clone();
        let source_l = source_l.clone();
        let lang = lang;
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title(t(lang, "Source PDF"))
                .modal(true)
                .build();
            let filter = gtk::FileFilter::new();
            filter.add_suffix("pdf");
            dialog.set_default_filter(Some(&filter));
            let source = source.clone();
            let source_l = source_l.clone();
            let win = win.clone();
            glib::spawn_future_local(async move {
                if let Ok(file) = dialog.open_future(Some(&win)).await {
                    if let Some(path) = file.path() {
                        source_l.set_text(&path.display().to_string());
                        *source.borrow_mut() = Some(path);
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
            let Some(path) = source.borrow().clone() else {
                show_toast(&overlay, t(lang, "Choose a source PDF."));
                return;
            };
            let src = match Document::open(&path) {
                Ok(d) => d,
                Err(e) => {
                    show_toast(&overlay, &format_core_error(&e));
                    return;
                }
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
            apply_in_place(&overlay, lang, &view, |doc| {
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
            apply_in_place(&overlay, lang, &view, |doc| crop_pages(doc, &ranges, crop));
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
            apply_in_place(&overlay, lang, &view, |doc| optimize(doc, &opts));
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
            apply_in_place(&overlay, lang, &view, |doc| protect(doc, &opts));
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
            apply_in_place(&overlay, lang, &view, |doc| unprotect(doc, pw.as_str()));
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
            apply_in_place(&overlay, lang, &view, sanitize_metadata);
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
            apply_in_place(&toast, lang, &view, |doc| apply_overlay(doc, &stamp));
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
        match pdf_to_rasters(&engine, &doc, 1.0) {
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
                    &openpdf_core::tr_args("wrote {path}", &[("path", &dir.display().to_string())]),
                );
            }
            Err(e) => show_toast(&overlay, &format_core_error(&e)),
        }
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
}
