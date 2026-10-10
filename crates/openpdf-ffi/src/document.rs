// SPDX-License-Identifier: MPL-2.0
use std::sync::{Arc, Mutex, MutexGuard};

use openpdf_core::{
    apply_overlay, crop_pages, extract, images_to_pdf, insert_pages, optimize, pdf_to_rasters,
    protect, replace_pages, sanitize_metadata, unprotect, CropBox, Document, Engine,
    OptimizeOptions, Overlay, OverlayKind, ProtectOptions, SignRequest, Signer, TileKey, TILE_SIZE,
};
use openpdf_sign::{Pkcs11Config, Pkcs11Signer};
use zeroize::Zeroizing;

use crate::error::FfiError;
use crate::pin::{ForeignPin, PinProvider};
use crate::types::{
    convert_ranges, Certificate, OutlineItem, PageInfo, PageRange, PageRaster, Permissions,
    RasterTile, SearchHit,
};

/// Process-wide library handle. Cheap to construct; PDFium is shared.
#[derive(uniffi::Object)]
pub struct OpenPdf {
    engine: Option<Engine>,
}

#[uniffi::export]
impl OpenPdf {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        crate::ensure_i18n();
        Arc::new(Self {
            engine: Engine::new().ok(),
        })
    }

    pub fn open(&self, path: String) -> Result<Arc<PdfDocument>, FfiError> {
        PdfDocument::open_with_engine(self.engine.clone(), path)
    }

    pub fn images_to_pdf(&self, paths: Vec<String>) -> Result<Arc<PdfDocument>, FfiError> {
        PdfDocument::from_images(self.engine.clone(), paths)
    }

    pub fn list_certificates(
        &self,
        pin: Box<dyn PinProvider>,
    ) -> Result<Vec<Certificate>, FfiError> {
        list_certificates(pin)
    }
}

/// One open PDF. Wraps `Mutex<Document>` plus the shared [`Engine`].
///
/// All strings and buffers crossing this API are owned (`String`, `Vec<u8>`).
#[derive(uniffi::Object)]
pub struct PdfDocument {
    engine: Option<Engine>,
    inner: Mutex<Document>,
}

#[uniffi::export]
impl PdfDocument {
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Arc<Self>, FfiError> {
        crate::ensure_i18n();
        Self::open_with_engine(Engine::new().ok(), path)
    }

    pub fn page_count(&self) -> Result<u32, FfiError> {
        Ok(self.lock()?.page_count())
    }

    pub fn pages(&self) -> Result<Vec<PageInfo>, FfiError> {
        Ok(self
            .lock()?
            .pages()
            .into_iter()
            .map(PageInfo::from)
            .collect())
    }

    pub fn path(&self) -> Result<Option<String>, FfiError> {
        Ok(self
            .lock()?
            .path()
            .map(|p| p.to_string_lossy().into_owned()))
    }

    pub fn is_dirty(&self) -> Result<bool, FfiError> {
        Ok(self.lock()?.is_dirty())
    }

    pub fn is_signed(&self) -> Result<bool, FfiError> {
        Ok(self.lock()?.is_signed())
    }

    pub fn is_encrypted(&self) -> Result<bool, FfiError> {
        Ok(self.lock()?.is_encrypted())
    }

    pub fn is_read_only(&self) -> Result<bool, FfiError> {
        Ok(self.lock()?.is_read_only())
    }

    pub fn can_undo(&self) -> Result<bool, FfiError> {
        Ok(self.lock()?.can_undo())
    }

    pub fn can_redo(&self) -> Result<bool, FfiError> {
        Ok(self.lock()?.can_redo())
    }

    /// Render one cache tile via [`Engine::render_tile_blocking`].
    pub fn render_tile(
        &self,
        page: u32,
        zoom: f32,
        tx: u32,
        ty: u32,
    ) -> Result<RasterTile, FfiError> {
        let doc = self.lock()?;
        let engine = self.require_engine()?;
        engine.bind(&doc)?;
        let key = TileKey::new(doc.id(), page, zoom, tx, ty);
        let tile = engine.render_tile_blocking(key)?;
        Ok(RasterTile::from_tile(tile))
    }

    pub fn page_text(&self, page: u32) -> Result<String, FfiError> {
        let doc = self.lock()?;
        let engine = self.require_engine()?;
        engine.bind(&doc)?;
        Ok(engine.page_text(doc.id(), page)?)
    }

    pub fn search(&self, query: String) -> Result<Vec<SearchHit>, FfiError> {
        let doc = self.lock()?;
        let engine = self.require_engine()?;
        engine.bind(&doc)?;
        Ok(engine
            .search(doc.id(), &query)?
            .into_iter()
            .map(SearchHit::from)
            .collect())
    }

    pub fn outline(&self) -> Result<Vec<OutlineItem>, FfiError> {
        Ok(self
            .lock()?
            .outline()?
            .into_iter()
            .map(OutlineItem::from)
            .collect())
    }

    pub fn rotate(&self, index: u32, degrees: i32) -> Result<(), FfiError> {
        let mut doc = self.lock()?;
        doc.rotate_page(index, degrees)?;
        self.refresh(&doc)
    }

    pub fn delete_page(&self, index: u32) -> Result<(), FfiError> {
        let mut doc = self.lock()?;
        doc.delete_page(index)?;
        self.refresh(&doc)
    }

    pub fn move_page(&self, from: u32, to: u32) -> Result<(), FfiError> {
        let mut doc = self.lock()?;
        doc.move_page(from, to)?;
        self.refresh(&doc)
    }

    pub fn undo(&self) -> Result<bool, FfiError> {
        let mut doc = self.lock()?;
        let changed = doc.undo()?;
        if changed {
            self.refresh(&doc)?;
        }
        Ok(changed)
    }

    pub fn redo(&self) -> Result<bool, FfiError> {
        let mut doc = self.lock()?;
        let changed = doc.redo()?;
        if changed {
            self.refresh(&doc)?;
        }
        Ok(changed)
    }

    /// Rewrite save, or incremental when the document is signed.
    pub fn save(&self, path: String) -> Result<(), FfiError> {
        let mut doc = self.lock()?;
        if doc.is_signed() {
            doc.save_incremental(&path)?;
        } else {
            doc.save(&path)?;
        }
        self.refresh(&doc)
    }

    pub fn extract(&self, ranges: Vec<PageRange>) -> Result<Arc<Self>, FfiError> {
        let ranges = convert_ranges(&ranges)?;
        let doc = self.lock()?;
        self.wrap_new(extract(&doc, &ranges)?)
    }

    pub fn insert_pages(
        &self,
        at: u32,
        source: Arc<PdfDocument>,
        ranges: Vec<PageRange>,
    ) -> Result<Arc<Self>, FfiError> {
        let ranges = convert_ranges(&ranges)?;
        let (target, src) = snapshot_pair(self, source.as_ref())?;
        self.wrap_new(insert_pages(&target, at, &src, &ranges)?)
    }

    pub fn replace_pages(
        &self,
        start: u32,
        count: u32,
        source: Arc<PdfDocument>,
        ranges: Vec<PageRange>,
    ) -> Result<Arc<Self>, FfiError> {
        let ranges = convert_ranges(&ranges)?;
        let (target, src) = snapshot_pair(self, source.as_ref())?;
        self.wrap_new(replace_pages(&target, start, count, &src, &ranges)?)
    }

    pub fn crop(
        &self,
        llx: f32,
        lly: f32,
        urx: f32,
        ury: f32,
        ranges: Vec<PageRange>,
    ) -> Result<Arc<Self>, FfiError> {
        let crop = CropBox::new(llx, lly, urx, ury)?;
        let ranges = convert_ranges(&ranges)?;
        let doc = self.lock()?;
        self.wrap_new(crop_pages(&doc, &ranges, crop)?)
    }

    pub fn optimize(
        &self,
        jpeg_quality: Option<u8>,
        drop_unused: bool,
        linearize: bool,
    ) -> Result<Arc<Self>, FfiError> {
        let opts = OptimizeOptions {
            jpeg_quality,
            drop_unused,
            linearize,
        };
        let doc = self.lock()?;
        self.wrap_new(optimize(&doc, &opts)?)
    }

    /// AES-256 encrypt. Passwords are wrapped in [`Zeroizing`] and never logged.
    pub fn protect(
        &self,
        user_password: String,
        owner_password: String,
        permissions: Option<Permissions>,
    ) -> Result<Arc<Self>, FfiError> {
        let user = Zeroizing::new(user_password);
        let owner = Zeroizing::new(owner_password);
        let secrets = [user.as_str(), owner.as_str()];
        let mut opts = ProtectOptions::new(user.as_str(), owner.as_str())
            .map_err(|e| FfiError::from_core_hiding(e, &secrets))?;
        if let Some(p) = permissions {
            opts.permissions = p.to_core();
        }
        let doc = self.lock().map_err(|e| e.hide_secrets(&secrets))?;
        let out = protect(&doc, &opts).map_err(|e| FfiError::from_core_hiding(e, &secrets))?;
        self.wrap_new(out).map_err(|e| e.hide_secrets(&secrets))
    }

    /// Decrypt with the user or owner password. The password is never logged.
    pub fn unprotect(&self, password: String) -> Result<Arc<Self>, FfiError> {
        let password = Zeroizing::new(password);
        let secrets = [password.as_str()];
        let doc = self.lock().map_err(|e| e.hide_secrets(&secrets))?;
        let out = unprotect(&doc, password.as_str())
            .map_err(|e| FfiError::from_core_hiding(e, &secrets))?;
        self.wrap_new(out).map_err(|e| e.hide_secrets(&secrets))
    }

    pub fn sanitize(&self) -> Result<Arc<Self>, FfiError> {
        let doc = self.lock()?;
        self.wrap_new(sanitize_metadata(&doc)?)
    }

    pub fn watermark(&self, text: String, ranges: Vec<PageRange>) -> Result<Arc<Self>, FfiError> {
        self.overlay(OverlayKind::Watermark, text, ranges)
    }

    pub fn header(&self, text: String, ranges: Vec<PageRange>) -> Result<Arc<Self>, FfiError> {
        self.overlay(OverlayKind::Header, text, ranges)
    }

    pub fn footer(&self, text: String, ranges: Vec<PageRange>) -> Result<Arc<Self>, FfiError> {
        self.overlay(OverlayKind::Footer, text, ranges)
    }

    pub fn page_numbers(
        &self,
        format: String,
        ranges: Vec<PageRange>,
    ) -> Result<Arc<Self>, FfiError> {
        self.overlay(OverlayKind::PageNumber, format, ranges)
    }

    pub fn to_rasters(&self, zoom: f32) -> Result<Vec<PageRaster>, FfiError> {
        let doc = self.lock()?;
        let engine = self.require_engine()?;
        Ok(pdf_to_rasters(engine, &doc, zoom)?
            .into_iter()
            .map(PageRaster::from)
            .collect())
    }

    /// PKCS#11 PAdES sign via [`openpdf_sign::Pkcs11Signer`]. The PIN comes
    /// from the foreign [`PinProvider`] dialog — never as a string argument.
    pub fn sign(
        &self,
        pin: Box<dyn PinProvider>,
        cert_id: String,
        reason: Option<String>,
        location: Option<String>,
    ) -> Result<(), FfiError> {
        let signer = pkcs11_signer(pin)?;
        let request = SignRequest {
            cert_id,
            reason,
            location,
            contact: None,
            appearance: None,
        };
        let mut doc = self.lock()?;
        doc.apply_signer(&signer, &request)?;
        self.refresh(&doc)
    }
}

impl PdfDocument {
    pub(crate) fn open_with_engine(
        engine: Option<Engine>,
        path: String,
    ) -> Result<Arc<Self>, FfiError> {
        let doc = if let Some(ref engine) = engine {
            engine.open(&path)?
        } else {
            Document::open(&path)?
        };
        Ok(Arc::new(Self {
            engine,
            inner: Mutex::new(doc),
        }))
    }

    pub(crate) fn from_images(
        engine: Option<Engine>,
        paths: Vec<String>,
    ) -> Result<Arc<Self>, FfiError> {
        let doc = images_to_pdf(&paths)?;
        Self::wrap(engine, doc)
    }

    fn wrap(engine: Option<Engine>, doc: Document) -> Result<Arc<Self>, FfiError> {
        if let Some(ref engine) = engine {
            // Encrypted files may fail PDFium bind until unprotect; keep the document.
            let _ = engine.bind(&doc);
        }
        Ok(Arc::new(Self {
            engine,
            inner: Mutex::new(doc),
        }))
    }

    fn wrap_new(&self, doc: Document) -> Result<Arc<Self>, FfiError> {
        Self::wrap(self.engine.clone(), doc)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Document>, FfiError> {
        self.inner.lock().map_err(|_| FfiError::lock_poisoned())
    }

    fn require_engine(&self) -> Result<&Engine, FfiError> {
        self.engine
            .as_ref()
            .ok_or_else(FfiError::engine_unavailable)
    }

    fn refresh(&self, doc: &Document) -> Result<(), FfiError> {
        if let Some(ref engine) = self.engine {
            engine.refresh(doc)?;
        }
        Ok(())
    }

    fn overlay(
        &self,
        kind: OverlayKind,
        text: String,
        ranges: Vec<PageRange>,
    ) -> Result<Arc<Self>, FfiError> {
        let mut overlay = Overlay::new(kind, text)?;
        overlay.pages = convert_ranges(&ranges)?;
        let doc = self.lock()?;
        self.wrap_new(apply_overlay(&doc, &overlay)?)
    }
}

/// Clone both documents out of their mutexes so insert/replace never holds
/// two guards (same `Arc` used to deadlock on the same mutex).
fn snapshot_pair(a: &PdfDocument, b: &PdfDocument) -> Result<(Document, Document), FfiError> {
    if std::ptr::eq(a, b) {
        let guard = a.lock()?;
        return Ok((guard.clone(), guard.clone()));
    }
    let (first, second) = if std::ptr::from_ref(a) < std::ptr::from_ref(b) {
        let ga = a.lock()?;
        let gb = b.lock()?;
        (ga.clone(), gb.clone())
    } else {
        let gb = b.lock()?;
        let ga = a.lock()?;
        (ga.clone(), gb.clone())
    };
    Ok((first, second))
}

#[uniffi::export]
pub fn list_certificates(pin: Box<dyn PinProvider>) -> Result<Vec<Certificate>, FfiError> {
    crate::ensure_i18n();
    let signer = pkcs11_signer(pin)?;
    Ok(signer
        .list_certificates()?
        .into_iter()
        .map(Certificate::from)
        .collect())
}

#[uniffi::export]
pub fn images_to_pdf_paths(paths: Vec<String>) -> Result<Arc<PdfDocument>, FfiError> {
    crate::ensure_i18n();
    PdfDocument::from_images(Engine::new().ok(), paths)
}

#[uniffi::export]
pub fn tile_size() -> u32 {
    TILE_SIZE
}

/// Create a signer per request. Failures are not cached so plugging a
/// reader in later can succeed.
fn pkcs11_signer(pin: Box<dyn PinProvider>) -> Result<Pkcs11Signer, FfiError> {
    Ok(Pkcs11Signer::new(
        Pkcs11Config::default(),
        ForeignPin::new(pin),
    )?)
}
