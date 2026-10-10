// SPDX-License-Identifier: MPL-2.0
//! DrawingArea that never snapshots an invalid cairo surface.
//!
//! GTK 4.14.5 `GtkDrawingArea` always calls `gtk_snapshot_append_cairo` with
//! the **full** widget allocation. Under GSK that creates a cairo image
//! surface of that size:
//!
//! * 0-wide allocation → `gdk_texture_new_for_surface` width 0 →
//!   `gsk_gl_driver_cache_texture` abort.
//! * height or width × scale > 32767 (32 fit-width A4 pages at 1280 px is
//!   ~56k tall) → `CAIRO_STATUS_INVALID_SIZE` →
//!   `gsk_render_node_draw: cairo_status == SUCCESS` CRITICAL.
//!
//! This widget paints only the visible scrolled rect, capped to cairo's max.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

use crate::paint;

type DrawFn = Rc<dyn Fn(&cairo::Context, i32, i32)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub draw: RefCell<Option<DrawFn>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "OpenPdfCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::DrawingArea;
    }

    impl ObjectImpl for Canvas {}
    impl WidgetImpl for Canvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let Some((x, y, w, h)) = visible_paint_rect(obj.upcast_ref()) else {
                return;
            };
            let Some(cr) = paint::append_cairo(snapshot, x as f32, y as f32, w as f32, h as f32)
            else {
                return;
            };
            if !paint::context_ok(&cr) {
                return;
            }
            cr.translate(-x, -y);
            if let Some(draw) = self.draw.borrow().as_ref() {
                draw(&cr, obj.width(), obj.height());
            }
            // Never finish() — GSK owns this context for the cairo node.
            if !paint::context_ok(&cr) {
                return;
            }
        }
    }
    impl DrawingAreaImpl for Canvas {}
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::DrawingArea, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Canvas {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_draw<F>(&self, f: F)
    where
        F: Fn(&cairo::Context, i32, i32) + 'static,
    {
        *self.imp().draw.borrow_mut() = Some(Rc::new(f));
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

/// Visible rect in widget coordinates, capped so device pixels fit cairo.
pub fn visible_paint_rect(widget: &gtk::Widget) -> Option<(f64, f64, f64, f64)> {
    let alloc_w = f64::from(widget.width());
    let alloc_h = f64::from(widget.height());
    if alloc_w <= 0.0 || alloc_h <= 0.0 {
        return None;
    }
    let scale = widget.scale_factor();
    let (x, y, w, h) = if let Some((vx, vy, vw, vh)) = scrolled_visible(widget) {
        let x = vx.clamp(0.0, alloc_w);
        let y = vy.clamp(0.0, alloc_h);
        let w = vw.min(alloc_w - x);
        let h = vh.min(alloc_h - y);
        (x, y, w, h)
    } else {
        (0.0, 0.0, alloc_w, alloc_h)
    };
    paint::clamp_cairo_rect(x, y, w, h, scale)
}

fn scrolled_visible(widget: &gtk::Widget) -> Option<(f64, f64, f64, f64)> {
    let mut parent = widget.parent();
    while let Some(p) = parent {
        if let Ok(scroll) = p.clone().downcast::<gtk::ScrolledWindow>() {
            let hadj = scroll.hadjustment();
            let vadj = scroll.vadjustment();
            let vw = hadj.page_size();
            let vh = vadj.page_size();
            if vw < 1.0 || vh < 1.0 {
                return None;
            }
            return Some((hadj.value(), vadj.value(), vw, vh));
        }
        parent = p.parent();
    }
    None
}
