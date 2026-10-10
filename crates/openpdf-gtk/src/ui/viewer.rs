// SPDX-License-Identifier: MPL-2.0
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::i18n::t;
use crate::measure::{MIN_THUMB_TILES, MIN_VISIBLE_TILES};
use crate::session::Opened;
use crate::settings::{PageLayoutMode, Settings, ZoomMode};
use crate::tiles::{
    layout_continuous, layout_single, visible_tiles, PageLayout, TileKey, DEFAULT_TILE_SIZE,
};
use crate::{Lang, PageSize};
use gtk::cairo;
use gtk::glib::{self, clone};
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use openpdf_core::Priority;

const PADDING: f64 = 18.0;
const GAP: f64 = 14.0;
const THUMB_WIDTH: f64 = 132.0;

struct Shared {
    doc: Rc<Opened>,
    current_page: Cell<u32>,
    zoom: Cell<f64>,
    zoom_mode: Cell<ZoomMode>,
    layout_mode: Cell<PageLayoutMode>,
    tile_size: Cell<u32>,
    layout: RefCell<PageLayout>,
    thumb_layout: RefCell<PageLayout>,
    last_visible: RefCell<Vec<TileKey>>,
    last_thumbs: RefCell<Vec<TileKey>>,
}

pub struct DocumentView {
    pub root: gtk::Widget,
    shared: Rc<Shared>,
    pages_scroll: gtk::ScrolledWindow,
    pages_area: gtk::DrawingArea,
    thumbs_scroll: gtk::ScrolledWindow,
    thumbs_area: gtk::DrawingArea,
    split: adw::OverlaySplitView,
    page_label: gtk::Label,
    zoom_label: gtk::Label,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    rotate_btn: gtk::Button,
    delete_btn: gtk::Button,
    prev_btn: gtk::Button,
    next_btn: gtk::Button,
    zoom_out_btn: gtk::Button,
    zoom_in_btn: gtk::Button,
    locked_banner: gtk::Label,
    lang: Lang,
}

impl DocumentView {
    pub fn widget(&self) -> &gtk::Widget {
        &self.root
    }

    pub fn opened(&self) -> &Opened {
        &self.shared.doc
    }

    pub fn title(&self) -> String {
        self.shared.doc.title()
    }

    pub fn path(&self) -> Option<std::path::PathBuf> {
        self.shared.doc.path()
    }

    pub fn zoom_in(&self) {
        self.set_percent_zoom((self.shared.zoom.get() * 1.15).min(8.0));
    }

    pub fn zoom_out(&self) {
        self.set_percent_zoom((self.shared.zoom.get() / 1.15).max(0.2));
    }

    pub fn toggle_sidebar(&self) {
        self.split.set_show_sidebar(!self.split.shows_sidebar());
    }

    pub fn set_sidebar(&self, show: bool) {
        self.split.set_show_sidebar(show);
    }

    pub fn reveal_find(&self) {
        self.search_bar.set_search_mode(true);
        self.search_entry.grab_focus();
    }

    pub fn next_page(&self) {
        let n = self.shared.doc.page_count() as u32;
        if n == 0 {
            return;
        }
        self.go_to_page((self.shared.current_page.get() + 1).min(n - 1));
    }

    pub fn prev_page(&self) {
        self.go_to_page(self.shared.current_page.get().saturating_sub(1));
    }

    pub fn rotate_current(&self, delta: i32) -> Result<(), String> {
        self.shared
            .doc
            .rotate(self.shared.current_page.get(), delta)
            .map_err(|e| e.to_string())?;
        self.relayout();
        Ok(())
    }

    pub fn delete_current(&self) -> Result<(), String> {
        self.shared
            .doc
            .delete_page(self.shared.current_page.get())
            .map_err(|e| e.to_string())?;
        let n = self.shared.doc.page_count() as u32;
        if n > 0 {
            self.shared
                .current_page
                .set(self.shared.current_page.get().min(n - 1));
        }
        self.relayout();
        Ok(())
    }

    pub fn undo(&self) -> Result<bool, String> {
        let changed = self.shared.doc.undo().map_err(|e| e.to_string())?;
        if changed {
            self.relayout();
        }
        Ok(changed)
    }

    pub fn redo(&self) -> Result<bool, String> {
        let changed = self.shared.doc.redo().map_err(|e| e.to_string())?;
        if changed {
            self.relayout();
        }
        Ok(changed)
    }

    pub fn refresh_lock_chrome(&self) {
        let locked = self.shared.doc.is_locked();
        self.locked_banner.set_visible(locked);
        self.rotate_btn.set_sensitive(!locked);
        self.delete_btn.set_sensitive(!locked);
    }

    pub fn queue_repaint(&self) {
        self.pages_area.queue_draw();
        self.thumbs_area.queue_draw();
    }

    /// Kick the top-left visible tile as soon as the document is open, without
    /// waiting for the 30 ms relayout timer or the first draw.
    pub fn request_first_tile(&self) {
        let keys = page_tile_keys(self);
        let key = keys.first().copied().unwrap_or_else(|| {
            TileKey::new(
                0,
                self.shared.zoom.get().max(1.0),
                0,
                0,
                self.shared.tile_size.get(),
            )
        });
        self.shared.doc.request_tiles(&[key], Priority::Visible);
        if self.shared.doc.poll_one() {
            self.pages_area.queue_draw();
        }
        let doc = self.shared.doc.clone();
        let area = self.pages_area.clone();
        let spins = Rc::new(Cell::new(0u32));
        glib::idle_add_local_full(glib::Priority::HIGH, move || {
            // Ingest at most the first tile, then wait for it to paint.
            // Polling the rest here would coalesce queue_draw with later tiles.
            if crate::measure::ui_first_tile_painted_ms() > 0 {
                return glib::ControlFlow::Break;
            }
            if crate::measure::ui_first_tile_rendered_ms() == 0 && doc.poll_one() {
                area.queue_draw();
            } else if crate::measure::ui_first_tile_rendered_ms() > 0 {
                area.queue_draw();
            }
            let n = spins.get() + 1;
            spins.set(n);
            if crate::measure::ui_first_tile_painted_ms() > 0 || n > 120 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    pub fn ensure_laid_out(&self) {
        self.relayout();
    }

    pub fn viewport_allocated(&self) -> bool {
        self.pages_scroll.height() > 64 && self.thumbs_scroll.height() > 32
    }

    /// Give the page canvas and thumbnail strip a real size under Xvfb.
    pub fn prepare_measure_viewport(&self) {
        self.set_sidebar(true);
        self.thumbs_scroll.set_min_content_height(480);
        self.pages_scroll.set_min_content_height(600);
        self.ensure_laid_out();
        self.queue_repaint();
    }

    /// Count cached tiles for the current page viewport and the thumbnail strip.
    ///
    /// Does not `poll()` — draining the render queue here would mark
    /// `ui_all_visible` in the same draw as `ui_first_tile`.
    pub fn tile_ready(&self) -> TileReady {
        let visible = page_tile_keys(self);
        let thumbs = thumb_tile_keys(self);
        TileReady {
            visible_ready: visible
                .iter()
                .filter(|k| self.shared.doc.paint_tile(k).is_some())
                .count(),
            visible_total: visible.len(),
            thumb_ready: thumbs
                .iter()
                .filter(|k| self.shared.doc.paint_tile(k).is_some())
                .count(),
            thumb_total: thumbs.len(),
        }
    }

    fn set_percent_zoom(&self, zoom: f64) {
        self.shared.zoom.set(zoom);
        self.shared.zoom_mode.set(ZoomMode::Percent);
        self.relayout();
        self.update_chrome();
    }

    /// Apply on-disk zoom after `hydrate_settings`. CLI `HANDLES_OPEN` used to
    /// build the viewer with `Settings::default()` (Fit Width) before the
    /// file was read, so 147% never reached the canvas.
    pub fn apply_zoom_settings(&self, settings: &Settings) {
        match settings.zoom_mode {
            ZoomMode::Percent => self.set_percent_zoom(settings.zoom_factor()),
            mode => {
                self.shared.zoom_mode.set(mode);
                self.relayout();
                self.update_chrome();
            }
        }
        self.set_sidebar(settings.show_thumbnails);
    }

    fn go_to_page(&self, page: u32) {
        let n = self.shared.doc.page_count() as u32;
        if n == 0 {
            return;
        }
        let page = page.min(n - 1);
        self.shared.current_page.set(page);
        if self.shared.layout_mode.get() == PageLayoutMode::Single {
            self.relayout();
        } else if let Some(rect) = self
            .shared
            .layout
            .borrow()
            .pages
            .iter()
            .find(|p| p.page == page)
        {
            self.pages_scroll.vadjustment().set_value(rect.y - 8.0);
        }
        self.thumbs_area.queue_draw();
        self.update_chrome();
    }

    fn relayout(&self) {
        let alloc_w = self.pages_scroll.width().max(1) as f64;
        let alloc_h = self.pages_scroll.height().max(1) as f64;
        let sizes = self.shared.doc.pages();
        if sizes.is_empty() {
            return;
        }
        let mut zoom = self.shared.zoom.get();
        match self.shared.zoom_mode.get() {
            ZoomMode::FitWidth => {
                zoom = ((alloc_w - PADDING * 2.0) / sizes[0].width).max(0.1);
            }
            ZoomMode::FitPage => {
                let zw = (alloc_w - PADDING * 2.0) / sizes[0].width;
                let zh = (alloc_h - PADDING * 2.0) / sizes[0].height;
                zoom = zw.min(zh).max(0.1);
            }
            ZoomMode::Percent => {}
        }
        self.shared.zoom.set(zoom);
        self.shared
            .doc
            .prefetch_first_tile(zoom, self.shared.tile_size.get());
        let layout = match self.shared.layout_mode.get() {
            PageLayoutMode::Continuous => layout_continuous(&sizes, zoom, alloc_w, PADDING, GAP),
            PageLayoutMode::Single => layout_single(
                &sizes,
                self.shared.current_page.get(),
                zoom,
                alloc_w,
                alloc_h,
                PADDING,
            ),
        };
        self.pages_area
            .set_content_width(layout.content_width.ceil() as i32);
        self.pages_area
            .set_content_height(layout.content_height.ceil() as i32);
        *self.shared.layout.borrow_mut() = layout;

        let thumb_zoom = THUMB_WIDTH / sizes[0].width;
        let thumbs = layout_continuous(&sizes, thumb_zoom, THUMB_WIDTH + 24.0, 10.0, 10.0);
        self.thumbs_area
            .set_content_width(thumbs.content_width.ceil() as i32);
        self.thumbs_area
            .set_content_height(thumbs.content_height.ceil() as i32);
        *self.shared.thumb_layout.borrow_mut() = thumbs;
        self.pages_area.queue_draw();
        self.thumbs_area.queue_draw();
        self.update_chrome();
        self.refresh_lock_chrome();
    }

    fn update_chrome(&self) {
        let n = self.shared.doc.page_count();
        let p = self.shared.current_page.get() + 1;
        self.page_label
            .set_text(&format!("{} {p} / {n}", t(self.lang, "Page")));
        self.zoom_label
            .set_text(&format!("{:.0}%", self.shared.zoom.get() * 100.0));
    }
}

/// How many of the on-screen page / thumbnail tiles are in the render cache.
#[derive(Clone, Copy, Debug)]
pub struct TileReady {
    pub visible_ready: usize,
    pub visible_total: usize,
    pub thumb_ready: usize,
    pub thumb_total: usize,
}

impl TileReady {
    /// Cache gate for `startup_ms` / `ui_all_visible_ms` only. Individual
    /// tiles `queue_draw` and paint as they arrive; this does not batch them.
    pub fn is_complete(self) -> bool {
        self.visible_total >= MIN_VISIBLE_TILES
            && self.visible_ready == self.visible_total
            && self.thumb_ready >= MIN_THUMB_TILES
            && self.thumb_total >= MIN_THUMB_TILES
    }
}

pub fn build(doc: Rc<Opened>, settings: &Settings, lang: Lang) -> Rc<DocumentView> {
    let tile_size = if settings.tile_size == 0 {
        DEFAULT_TILE_SIZE
    } else {
        settings.tile_size
    };
    let sizes = doc.pages();
    let zoom = settings.zoom_factor();
    let shared = Rc::new(Shared {
        current_page: Cell::new(0),
        zoom: Cell::new(zoom),
        zoom_mode: Cell::new(settings.zoom_mode),
        layout_mode: Cell::new(settings.page_layout),
        tile_size: Cell::new(tile_size),
        layout: RefCell::new(layout_continuous(&sizes, zoom, 800.0, PADDING, GAP)),
        thumb_layout: RefCell::new(layout_continuous(
            &sizes,
            THUMB_WIDTH / page_width_or_a4(&sizes),
            THUMB_WIDTH + 24.0,
            10.0,
            10.0,
        )),
        last_visible: RefCell::new(Vec::new()),
        last_thumbs: RefCell::new(Vec::new()),
        doc,
    });
    shared
        .doc
        .prefetch_first_tile(zoom, tile_size);

    let pages_area = gtk::DrawingArea::new();
    pages_area.add_css_class("page-canvas");
    pages_area.set_hexpand(true);
    pages_area.set_vexpand(true);
    pages_area.set_content_width(shared.layout.borrow().content_width.ceil() as i32);
    pages_area.set_content_height(shared.layout.borrow().content_height.ceil() as i32);

    let pages_scroll = gtk::ScrolledWindow::new();
    pages_scroll.set_hexpand(true);
    pages_scroll.set_vexpand(true);
    pages_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    pages_scroll.set_child(Some(&pages_area));

    let thumbs_area = gtk::DrawingArea::new();
    thumbs_area.add_css_class("thumb-canvas");
    thumbs_area.set_content_width((THUMB_WIDTH + 24.0) as i32);
    thumbs_area.set_content_height(shared.thumb_layout.borrow().content_height.ceil() as i32);

    let thumbs_scroll = gtk::ScrolledWindow::new();
    thumbs_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    thumbs_scroll.set_min_content_width((THUMB_WIDTH + 28.0) as i32);
    thumbs_scroll.set_min_content_height(240);
    thumbs_scroll.set_vexpand(true);
    thumbs_scroll.set_child(Some(&thumbs_area));

    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let side_header = gtk::Label::new(Some(t(lang, "Thumbnails")));
    side_header.add_css_class("caption");
    side_header.add_css_class("dim-label");
    side_header.set_margin_top(8);
    side_header.set_margin_bottom(4);
    sidebar.append(&side_header);
    sidebar.append(&thumbs_scroll);
    append_outline(&sidebar, &shared, lang);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let locked_banner = gtk::Label::new(Some(t(
        lang,
        "Signed document — read-only. Rotate, delete and reorder are disabled.",
    )));
    locked_banner.add_css_class("placeholder-banner");
    locked_banner.set_wrap(true);
    locked_banner.set_xalign(0.0);
    locked_banner.set_visible(shared.doc.is_locked());
    content.append(&locked_banner);

    if shared.doc.is_placeholder() {
        let banner = gtk::Label::new(Some(t(
            lang,
            "Preview uses a placeholder renderer because PDFium is not available.",
        )));
        banner.add_css_class("placeholder-banner");
        banner.set_wrap(true);
        banner.set_xalign(0.0);
        content.append(&banner);
    }

    let chrome = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    chrome.set_margin_start(8);
    chrome.set_margin_end(8);
    chrome.set_margin_top(6);
    chrome.set_margin_bottom(6);
    let prev = gtk::Button::new();
    prev.add_css_class("flat");
    let next = gtk::Button::new();
    next.add_css_class("flat");
    let page_label = gtk::Label::new(None);
    page_label.add_css_class("dim-label");
    let rotate_btn = gtk::Button::new();
    rotate_btn.add_css_class("flat");
    rotate_btn.set_tooltip_text(Some(t(lang, "Rotate page")));
    let delete_btn = gtk::Button::new();
    delete_btn.add_css_class("flat");
    delete_btn.set_tooltip_text(Some(t(lang, "Delete page")));
    let zoom_out = gtk::Button::new();
    zoom_out.add_css_class("flat");
    let zoom_in = gtk::Button::new();
    zoom_in.add_css_class("flat");
    let zoom_label = gtk::Label::new(None);
    zoom_label.add_css_class("dim-label");
    chrome.append(&prev);
    chrome.append(&page_label);
    chrome.append(&next);
    chrome.append(&rotate_btn);
    chrome.append(&delete_btn);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    chrome.append(&spacer);
    chrome.append(&zoom_out);
    chrome.append(&zoom_label);
    chrome.append(&zoom_in);

    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some(t(lang, "Find in document")));
    let search_bar = gtk::SearchBar::new();
    search_bar.set_child(Some(&search_entry));
    search_bar.set_show_close_button(true);
    search_bar.connect_entry(&search_entry);

    content.append(&chrome);
    content.append(&search_bar);
    content.append(&pages_scroll);

    let split = adw::OverlaySplitView::new();
    split.set_sidebar(Some(&sidebar));
    split.set_content(Some(&content));
    split.set_show_sidebar(settings.show_thumbnails);
    split.set_min_sidebar_width(THUMB_WIDTH + 32.0);
    split.set_max_sidebar_width(260.0);
    split.set_sidebar_position(gtk::PackType::Start);

    let view = Rc::new(DocumentView {
        root: split.clone().upcast(),
        shared: shared.clone(),
        pages_scroll: pages_scroll.clone(),
        pages_area: pages_area.clone(),
        thumbs_scroll: thumbs_scroll.clone(),
        thumbs_area: thumbs_area.clone(),
        split: split.clone(),
        page_label: page_label.clone(),
        zoom_label: zoom_label.clone(),
        search_bar: search_bar.clone(),
        search_entry: search_entry.clone(),
        rotate_btn: rotate_btn.clone(),
        delete_btn: delete_btn.clone(),
        prev_btn: prev.clone(),
        next_btn: next.clone(),
        zoom_out_btn: zoom_out.clone(),
        zoom_in_btn: zoom_in.clone(),
        locked_banner: locked_banner.clone(),
        lang,
    });
    view.update_chrome();
    view.refresh_lock_chrome();
    crate::measure::note_idle_load_pending();
    glib::idle_add_local_once(clone!(@strong view => move || {
        view.prev_btn.set_icon_name("go-up-symbolic");
        view.next_btn.set_icon_name("go-down-symbolic");
        view.rotate_btn.set_icon_name("object-rotate-right-symbolic");
        view.delete_btn.set_icon_name("edit-delete-symbolic");
        view.zoom_out_btn.set_icon_name("zoom-out-symbolic");
        view.zoom_in_btn.set_icon_name("zoom-in-symbolic");
        crate::measure::note_idle_load_finished();
    }));

    pages_area.set_draw_func(clone!(@strong view => move |_, cr, _, _| {
        paint_pages(cr, &view);
    }));
    thumbs_area.set_draw_func(clone!(@strong view => move |area, cr, _, _| {
        paint_thumbs(cr, &view);
        area.set_content_height(view.shared.thumb_layout.borrow().content_height.ceil() as i32);
    }));

    pages_scroll
        .vadjustment()
        .connect_value_changed(clone!(@strong view => move |adj| {
            sync_page_from_scroll(&view, adj.value());
            view.pages_area.queue_draw();
        }));
    pages_scroll
        .hadjustment()
        .connect_value_changed(clone!(@strong view => move |_| {
            view.pages_area.queue_draw();
        }));
    pages_scroll.connect_notify_local(
        Some("allocated-width"),
        clone!(@strong view => move |_, _| view.relayout()),
    );

    let click_pages = gtk::GestureClick::new();
    click_pages.connect_pressed(clone!(@strong view => move |_, _, _, y| {
        let y = y + view.pages_scroll.vadjustment().value();
        if let Some(page) = page_at(&view.shared.layout.borrow(), y) {
            view.shared.current_page.set(page);
            view.update_chrome();
            view.thumbs_area.queue_draw();
        }
    }));
    pages_area.add_controller(click_pages);

    let click_thumbs = gtk::GestureClick::new();
    click_thumbs.connect_pressed(clone!(@strong view => move |_, _, _, y| {
        let y = y + view.thumbs_scroll.vadjustment().value();
        if let Some(page) = page_at(&view.shared.thumb_layout.borrow(), y) {
            view.go_to_page(page);
        }
    }));
    thumbs_area.add_controller(click_thumbs);

    prev.connect_clicked(clone!(@strong view => move |_| view.prev_page()));
    next.connect_clicked(clone!(@strong view => move |_| view.next_page()));
    zoom_in.connect_clicked(clone!(@strong view => move |_| view.zoom_in()));
    zoom_out.connect_clicked(clone!(@strong view => move |_| view.zoom_out()));
    rotate_btn.connect_clicked(clone!(@strong view => move |_| {
        let _ = view.rotate_current(90);
    }));
    delete_btn.connect_clicked(clone!(@strong view => move |_| {
        let _ = view.delete_current();
    }));

    search_entry.connect_activate(clone!(@strong view => move |entry| {
        run_search(&view, &entry.text());
    }));

    glib::timeout_add_local_once(
        std::time::Duration::from_millis(30),
        clone!(@strong view => move || {
            view.relayout();
        }),
    );
    schedule_tile_pump(view.clone());

    view
}

/// One finished tile → one `queue_draw`. Yield after each arrival so GTK can
/// paint whatever is already in the cache instead of draining the queue first.
fn schedule_tile_pump(view: Rc<DocumentView>) {
    fn tick(view: Rc<DocumentView>) {
        glib::idle_add_local_full(
            glib::Priority::HIGH,
            clone!(@strong view => move || {
                let first_painted = crate::measure::ui_first_tile_painted_ms() > 0;
                let first_rendered = crate::measure::ui_first_tile_rendered_ms() > 0;
                // Hold the rest of the queue until the first tile is painted.
                if !first_painted && first_rendered {
                    view.pages_area.queue_draw();
                    let again = view.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(1),
                        move || tick(again),
                    );
                    return glib::ControlFlow::Break;
                }
                if view.shared.doc.poll_one() {
                    view.pages_area.queue_draw();
                    view.thumbs_area.queue_draw();
                    let again = view.clone();
                    let wait = if first_painted {
                        std::time::Duration::ZERO
                    } else {
                        std::time::Duration::from_millis(1)
                    };
                    glib::timeout_add_local_once(wait, move || {
                        tick(again);
                    });
                    glib::ControlFlow::Break
                } else {
                    let again = view.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(1), move || {
                        tick(again);
                    });
                    glib::ControlFlow::Break
                }
            }),
        );
    }
    tick(view);
}

fn append_outline(sidebar: &gtk::Box, shared: &Rc<Shared>, lang: Lang) {
    let items = shared.doc.outline();
    if items.is_empty() {
        return;
    }
    let label = gtk::Label::new(Some(t(lang, "Bookmarks")));
    label.add_css_class("caption");
    label.add_css_class("dim-label");
    label.set_margin_top(8);
    sidebar.append(&label);
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    fn add_items(list: &gtk::ListBox, items: &[openpdf_core::OutlineItem], depth: usize) {
        for item in items {
            let row = gtk::Label::new(Some(&item.title));
            row.set_xalign(0.0);
            row.set_margin_start(8 + (depth as i32) * 12);
            row.set_margin_top(4);
            row.set_margin_bottom(4);
            list.append(&row);
            add_items(list, &item.children, depth + 1);
        }
    }
    add_items(&list, &items, 0);
    sidebar.append(&list);
}

fn page_width_or_a4(sizes: &[PageSize]) -> f64 {
    sizes.first().map(|s| s.width).unwrap_or(PageSize::A4.width)
}

fn page_at(layout: &PageLayout, y: f64) -> Option<u32> {
    layout
        .pages
        .iter()
        .find(|p| y >= p.y && y <= p.y + p.height)
        .map(|p| p.page)
}

fn sync_page_from_scroll(view: &DocumentView, y: f64) {
    if view.shared.layout_mode.get() == PageLayoutMode::Single {
        return;
    }
    let mid = y + view.pages_scroll.vadjustment().page_size() / 2.0;
    if let Some(page) = view
        .shared
        .layout
        .borrow()
        .pages
        .iter()
        .find(|p| mid >= p.y && mid <= p.y + p.height)
        .map(|p| p.page)
    {
        if view.shared.current_page.get() != page {
            view.shared.current_page.set(page);
            view.update_chrome();
            view.thumbs_area.queue_draw();
        }
    }
}

fn page_tile_keys(view: &DocumentView) -> Vec<TileKey> {
    let vadj = view.pages_scroll.vadjustment();
    let hadj = view.pages_scroll.hadjustment();
    let view_x = hadj.value();
    let view_y = vadj.value();
    let view_w = hadj.page_size().max(view.pages_scroll.width() as f64);
    let view_h = vadj.page_size().max(1.0);
    visible_tiles(
        &view.shared.layout.borrow(),
        view_x,
        view_y,
        view_w,
        view_h,
        view.shared.zoom.get(),
        view.shared.tile_size.get(),
        1,
    )
}

fn thumb_tile_keys(view: &DocumentView) -> Vec<TileKey> {
    let layout = view.shared.thumb_layout.borrow();
    let vadj = view.thumbs_scroll.vadjustment();
    let allocated = vadj.page_size().max(view.thumbs_scroll.height() as f64);
    // Before the strip is allocated (first frames / Xvfb) `page_size` is 0, so
    // a 1 px viewport would request only page 1 and the rest never appear.
    // Once allocated, only the visible thumbs are created; off-screen tiles
    // are dropped in `paint_thumbs`.
    let cover_all = allocated < 32.0;
    let view_y = if cover_all { 0.0 } else { vadj.value() };
    let view_h = if cover_all {
        layout.content_height.max(1.0)
    } else {
        allocated.max(1.0)
    };
    let sizes = view.shared.doc.pages();
    let zoom = sizes.first().map(|s| THUMB_WIDTH / s.width).unwrap_or(0.2);
    visible_tiles(
        &layout,
        0.0,
        view_y,
        THUMB_WIDTH + 24.0,
        view_h,
        zoom,
        view.shared.tile_size.get(),
        1,
    )
}

fn paint_pages(cr: &cairo::Context, view: &DocumentView) {
    let layout = view.shared.layout.borrow();

    cr.set_source_rgb(0.78, 0.77, 0.75);
    let _ = cr.paint();

    let keys = page_tile_keys(view);
    if *view.shared.last_visible.borrow() != keys {
        view.shared.doc.begin_frame();
        *view.shared.last_visible.borrow_mut() = keys.clone();
    }
    view.shared.doc.request_tiles(&keys, Priority::Visible);
    let preview: Vec<TileKey> = keys.iter().map(|k| k.preview()).collect();
    view.shared.doc.request_tiles(&preview, Priority::Adjacent);

    for page in &layout.pages {
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.18);
        cr.rectangle(page.x + 3.0, page.y + 4.0, page.width, page.height);
        let _ = cr.fill();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(page.x, page.y, page.width, page.height);
        let _ = cr.fill();

        for key in keys.iter().filter(|k| k.page == page.page) {
            let (tile_x, tile_y) = tile_dest_origin(page.x, page.y, key.tx, key.ty, key.tile_size);
            if let Some(bmp) = view.shared.doc.paint_tile(key) {
                paint_rgba(cr, tile_x, tile_y, bmp.width, bmp.height, &bmp.rgba, true);
            } else if let Some(low) = view.shared.doc.paint_tile(&key.preview()) {
                let _ = cr.save();
                let _ = cr.translate(tile_x, tile_y);
                let sx = f64::from(key.tile_size) / f64::from(low.width.max(1));
                let sy = f64::from(key.tile_size) / f64::from(low.height.max(1));
                cr.scale(sx.min(4.0), sy.min(4.0));
                paint_rgba(cr, 0.0, 0.0, low.width, low.height, &low.rgba, false);
                let _ = cr.restore();
            }
        }
    }
    maybe_mark_startup(view);
}

fn paint_thumbs(cr: &cairo::Context, view: &DocumentView) {
    let layout = view.shared.thumb_layout.borrow();
    let current = view.shared.current_page.get();
    cr.set_source_rgb(0.94, 0.93, 0.91);
    let _ = cr.paint();

    let keys = thumb_tile_keys(view);
    if *view.shared.last_thumbs.borrow() != keys {
        let dropped: Vec<TileKey> = view
            .shared
            .last_thumbs
            .borrow()
            .iter()
            .filter(|k| !keys.contains(k))
            .copied()
            .collect();
        if !dropped.is_empty() {
            view.shared.doc.drop_tiles(&dropped);
        }
        *view.shared.last_thumbs.borrow_mut() = keys.clone();
    }
    view.shared.doc.request_tiles(&keys, Priority::Thumbnail);
    let mut keep = page_tile_keys(view);
    keep.extend(keys.iter().copied());
    let previews: Vec<TileKey> = keep.iter().map(|k| k.preview()).collect();
    keep.extend(previews);
    view.shared.doc.cancel_except(&keep);

    for page in &layout.pages {
        if page.page == current {
            cr.set_source_rgb(0.055, 0.486, 0.400);
            cr.rectangle(
                page.x - 3.0,
                page.y - 3.0,
                page.width + 6.0,
                page.height + 6.0,
            );
            let _ = cr.fill();
        }
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(page.x, page.y, page.width, page.height);
        let _ = cr.fill();
        for key in keys.iter().filter(|k| k.page == page.page) {
            let (tile_x, tile_y) = tile_dest_origin(page.x, page.y, key.tx, key.ty, key.tile_size);
            if let Some(bmp) = view.shared.doc.paint_tile(key) {
                paint_rgba(cr, tile_x, tile_y, bmp.width, bmp.height, &bmp.rgba, false);
            }
        }
        cr.set_source_rgb(0.25, 0.25, 0.26);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(11.0);
        let label = format!("{}", page.page + 1);
        if let Ok(ext) = cr.text_extents(&label) {
            let _ = cr.move_to(
                page.x + (page.width - ext.width()) / 2.0,
                page.y + page.height + 12.0,
            );
            let _ = cr.show_text(&label);
        }
    }
    maybe_mark_startup(view);
}

/// `TileReady::is_complete` gates `startup_ms` only, and only from the draw
/// path after tiles have actually been painted.
fn maybe_mark_startup(view: &DocumentView) {
    let ready = view.tile_ready();
    if ready.thumb_ready >= MIN_THUMB_TILES && ready.thumb_total >= MIN_THUMB_TILES {
        crate::measure::mark_thumbs_done();
    }
    if ready.is_complete() {
        crate::measure::mark_all_visible();
    }
}

/// Destination of a tile on the page canvas. Same origin as core `tile_rect`
/// (`tx * tile`, `ty * tile`) plus the page's layout offset.
pub(crate) fn tile_dest_origin(
    page_x: f64,
    page_y: f64,
    tx: u32,
    ty: u32,
    tile_size: u32,
) -> (f64, f64) {
    (
        page_x + f64::from(tx.saturating_mul(tile_size)),
        page_y + f64::from(ty.saturating_mul(tile_size)),
    )
}

/// Convert unpremultiplied RGBA8 to cairo ARgb32 (BGRA on little-endian).
///
/// Do not cache this by the source pointer: the tile cache recycles `Vec`
/// allocations, so a later tile can reuse the first tile's address and every
/// column would stamp the same pixels (`openPDF sample - paF sample - pa`).
fn rgba_to_cairo_argb(rgba: &[u8], width: u32, height: u32) -> Option<(Vec<u8>, i32)> {
    let w = width as i32;
    let h = height as i32;
    if w <= 0 || h <= 0 {
        return None;
    }
    let stride = cairo::Format::ARgb32.stride_for_width(width).ok()?;
    let mut data = vec![0u8; stride as usize * h as usize];
    let src_stride = width as usize * 4;
    for (dest_row, src_row) in data
        .chunks_mut(stride as usize)
        .zip(rgba.chunks_exact(src_stride))
        .take(height as usize)
    {
        let (dest_px, _) = dest_row.as_chunks_mut::<4>();
        let (src_px, _) = src_row.as_chunks::<4>();
        for (dest, src) in dest_px.iter_mut().zip(src_px) {
            dest[0] = src[2];
            dest[1] = src[1];
            dest[2] = src[0];
            dest[3] = src[3];
        }
    }
    Some((data, stride))
}

pub(crate) fn paint_rgba(
    cr: &cairo::Context,
    x: f64,
    y: f64,
    width: u32,
    height: u32,
    rgba: &[u8],
    real_page_tile: bool,
) {
    let Some((data, stride)) = rgba_to_cairo_argb(rgba, width, height) else {
        return;
    };
    if let Ok(surface) = cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        width as i32,
        height as i32,
        stride,
    ) {
        crate::measure::mark_first_surface_ready();
        let _ = cr.set_source_surface(&surface, x, y);
        let _ = cr.paint();
        if real_page_tile {
            crate::measure::mark_first_tile_painted();
        }
    }
}

fn run_search(view: &DocumentView, query: &str) {
    let q = query.trim();
    if q.is_empty() {
        return;
    }
    match view.shared.doc.search_pages(q) {
        Ok(pages) => {
            if let Some(&page) = pages.first() {
                view.go_to_page(page);
            } else {
                view.page_label.set_text(t(view.lang, "No matches"));
            }
        }
        Err(_) => view.page_label.set_text(t(view.lang, "No matches")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpdf_core::Engine;

    /// Recycled source pointer must not stamp the previous tile. The engine
    /// reuses `Vec` allocations; a cache keyed by `as_ptr()` then paints tile
    /// A into tile B's slot (run-to-run "openPDF sample - paF sample - pa").
    #[test]
    fn cairo_reused_rgba_pointer_does_not_stamp_previous_tile() {
        let w = 16u32;
        let h = 16u32;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, (w * 2) as i32, h as i32)
            .expect("reuse surface");
        {
            let cr = cairo::Context::new(&surf).unwrap();
            let (x0, y0) = tile_dest_origin(0.0, 0.0, 0, 0, w);
            paint_rgba(&cr, x0, y0, w, h, &rgba, false);
            for px in rgba.chunks_exact_mut(4) {
                px.copy_from_slice(&[0, 0, 255, 255]);
            }
            let (x1, y1) = tile_dest_origin(0.0, 0.0, 1, 0, w);
            paint_rgba(&cr, x1, y1, w, h, &rgba, false);
        }
        surf.flush();
        let data = surf.take_data().expect("reuse cairo data");
        let stride = cairo::Format::ARgb32.stride_for_width(w * 2).unwrap() as usize;
        // ARgb32 on LE is BGRA.
        let left = &data[0..4];
        let right = &data[16 * 4..16 * 4 + 4];
        let left_is_red = left[0] < 32 && left[1] < 32 && left[2] > 200;
        let right_is_blue = right[0] > 200 && right[1] < 32 && right[2] < 32;
        assert!(
            left_is_red && right_is_blue,
            "reused RGBA pointer stamped the previous tile (left={left:?} right={right:?} stride={stride})"
        );
    }

    /// UI placement only: draw each tile through `paint_rgba` at
    /// `tile_dest_origin` (the same helper `paint_pages` uses) and compare
    /// to one full-page `paint_rgba`. Tiles are copied into one scratch
    /// buffer so a pointer-keyed ARGB cache would reuse the first tile.
    /// Core `tile_rect` / `tile_grid` / `quantize_zoom` are not changed.
    #[test]
    fn cairo_tile_placement_matches_full_render_at_147_percent() {
        let engine = Engine::new().expect("PDFium required for 147% cairo placement");
        let path = crate::measure::write_sample_text_pdf(2).expect("sample text PDF");
        let session = crate::session::Session::open(&engine, &path).expect("open sample text PDF");
        let id = session.doc_id();
        let zoom = 1.47f32;
        let full = engine
            .render_page(id, 0, zoom)
            .expect("full page render at 147%");
        let tile = DEFAULT_TILE_SIZE;
        let (cols, rows) = crate::tiles::tile_grid(full.width, full.height, tile);
        assert!(
            cols >= 2 && rows >= 2,
            "147% page must span several tiles, got {cols}x{rows}"
        );

        let w = full.width as i32;
        let h = full.height as i32;
        let full_surf =
            cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).expect("full cairo surface");
        {
            let cr = cairo::Context::new(&full_surf).unwrap();
            paint_rgba(&cr, 0.0, 0.0, full.width, full.height, &full.rgba, false);
        }

        let tile_surf =
            cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).expect("tile cairo surface");
        {
            let cr = cairo::Context::new(&tile_surf).unwrap();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.paint();
            // One allocation for every tile: same pointer, different pixels.
            // A LAST-pointer cache then stamps column 0 into every full tile.
            let mut scratch = Vec::new();
            for ty in 0..rows {
                for tx in 0..cols {
                    let key = openpdf_core::TileKey::new(id, 0, zoom, tx, ty);
                    let rendered = engine.render_tile_blocking(key).expect("tile at 147%");
                    scratch.clear();
                    scratch.extend_from_slice(&rendered.rgba);
                    let (x, y) = tile_dest_origin(0.0, 0.0, tx, ty, tile);
                    paint_rgba(&cr, x, y, rendered.width, rendered.height, &scratch, false);
                }
            }
        }

        let _ = std::fs::remove_file(&path);
        full_surf.flush();
        tile_surf.flush();
        let full_bytes = full_surf.take_data().expect("full cairo data").to_vec();
        let tile_bytes = tile_surf.take_data().expect("tile cairo data").to_vec();
        let mismatch = crate::tiles::pixel_mismatch_ratio(&full_bytes, &tile_bytes, 12);
        assert!(
            mismatch < 0.01,
            "cairo tile placement at 147% diverges from full render ({:.2}% pixels, max Δ {})",
            mismatch * 100.0,
            crate::tiles::max_channel_delta(&full_bytes, &tile_bytes)
        );
    }
}
