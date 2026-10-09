//! OCR text-layer writing lives in this crate. Recognition is [`OcrEngine`].
//!
//! The application binary constructs `Box<dyn OcrEngine>` (like [`crate::Signer`])
//! and hands it to the UI / CLI. Linux default is Tesseract via a runtime
//! `tesseract` binary or `libtesseract` (`OPENPDF_TESSERACT`, PATH, Flatpak
//! `/app/extensions`). It is never linked. Windows.Media.Ocr and macOS Vision
//! implement the same trait from platform crates later.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Object, ObjectId, StringFormat};

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result, SignedOp};
use crate::ops::PageRange;
use crate::render::{Engine, Raster};

/// How Tesseract was found (for the UI capability query).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrBackend {
    Binary,
    Library,
    /// Platform crate (Windows.Media.Ocr, Vision, …).
    Platform,
}

/// UI-facing OCR probe. `available == false` means every other tool still works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcrCapability {
    pub available: bool,
    pub backend: Option<OcrBackend>,
    pub languages: Vec<String>,
}

impl OcrCapability {
    pub fn unavailable() -> Self {
        Self {
            available: false,
            backend: None,
            languages: Vec::new(),
        }
    }
}

/// One recognized word. Box is in raster pixels, origin top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// OCR settings. `language` is an engine lang code (`eng`, `spa`, …).
#[derive(Clone, Debug)]
pub struct OcrOptions {
    pub language: String,
    pub pages: Vec<PageRange>,
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            language: "eng".into(),
            pages: Vec::new(),
        }
    }
}

/// Recognize a page raster. Text-layer writing stays in [`ocr`].
///
/// ```ignore
/// let engine: Box<dyn openpdf_core::OcrEngine> = openpdf_core::default_ocr_engine();
/// // or, from a platform crate:
/// // let engine: Box<dyn openpdf_core::OcrEngine> = Box::new(windows_ocr::WindowsMediaOcr::new());
/// ```
pub trait OcrEngine: Send + Sync {
    fn capability(&self) -> OcrCapability;
    fn languages(&self) -> Vec<String>;
    fn recognize(&self, raster: &Raster, language: &str) -> Result<Vec<OcrWord>>;
}

/// Stub used when no engine is configured (Windows / macOS until a platform
/// crate supplies Windows.Media.Ocr or Vision).
#[derive(Clone, Debug)]
pub struct UnavailableOcr {
    hint: String,
}

impl UnavailableOcr {
    pub fn new(hint: impl Into<String>) -> Self {
        Self { hint: hint.into() }
    }
}

impl OcrEngine for UnavailableOcr {
    fn capability(&self) -> OcrCapability {
        OcrCapability::unavailable()
    }

    fn languages(&self) -> Vec<String> {
        Vec::new()
    }

    fn recognize(&self, _raster: &Raster, _language: &str) -> Result<Vec<OcrWord>> {
        Err(Error::invalid(InvalidKind::OcrUnavailable {
            hint: self.hint.clone(),
        }))
    }
}

/// Tesseract via `tesseract` / `libtesseract` (never linked). Linux default.
pub struct TesseractOcr {
    inner: TessEngine,
}

impl TesseractOcr {
    pub fn probe() -> Option<Self> {
        resolve_engine().map(|inner| Self { inner })
    }
}

impl OcrEngine for TesseractOcr {
    fn capability(&self) -> OcrCapability {
        OcrCapability {
            available: true,
            backend: Some(self.inner.backend()),
            languages: self.inner.list_languages(),
        }
    }

    fn languages(&self) -> Vec<String> {
        self.inner.list_languages()
    }

    fn recognize(&self, raster: &Raster, language: &str) -> Result<Vec<OcrWord>> {
        self.inner.recognize(raster, language)
    }
}

/// Linux: Tesseract if present. Other OS: Tesseract only when found (CLI),
/// otherwise [`UnavailableOcr`] so a platform crate can supply the engine.
pub fn default_ocr_engine() -> Box<dyn OcrEngine> {
    if let Some(t) = TesseractOcr::probe() {
        return Box::new(t);
    }
    Box::new(UnavailableOcr::new(ocr_install_hint()))
}

/// Probe the default engine without failing the rest of the crate.
pub fn ocr_capability() -> OcrCapability {
    default_ocr_engine().capability()
}

/// Add an invisible searchable text layer. Signed documents are refused.
pub fn ocr(
    render: &Engine,
    ocr_engine: &dyn OcrEngine,
    doc: &Document,
    opts: &OcrOptions,
) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Edit));
    }
    let cap = ocr_engine.capability();
    if !cap.available {
        return Err(Error::invalid(InvalidKind::OcrUnavailable {
            hint: ocr_install_hint(),
        }));
    }
    let langs = ocr_engine.languages();
    if !langs.is_empty() && !langs.iter().any(|l| l == &opts.language) {
        return Err(Error::invalid(InvalidKind::OcrLanguageMissing {
            language: opts.language.clone(),
            installed: langs.join(", "),
            hint: language_install_hint(&opts.language),
        }));
    }

    render.bind(doc)?;
    let mut inner = doc.assembled_lopdf()?;
    let page_map = inner.get_pages();
    let total = doc.page_count();
    let mut indices: Vec<u32> = if opts.pages.is_empty() {
        (0..total).collect()
    } else {
        opts.pages.iter().flat_map(|r| r.iter()).collect()
    };
    indices.sort_unstable();
    indices.dedup();

    let font_id = helvetica_winansi(&mut inner);
    let zoom = 150.0 / 72.0;

    for i in indices {
        let page_id = page_map
            .get(&(i + 1))
            .copied()
            .ok_or(Error::PageOutOfRange {
                page: i,
                count: total,
            })?;
        ensure_font(&mut inner, page_id, font_id)?;
        let (pw, ph) = page_size(&inner, page_id).unwrap_or((612.0, 792.0));
        let raster = render.render_page(doc.id(), i, zoom)?;
        let words = ocr_engine.recognize(&raster, &opts.language)?;
        if words.is_empty() {
            continue;
        }
        let ops = invisible_text_ops(&words, raster.width, raster.height, pw, ph);
        let bytes = Content { operations: ops }
            .encode()
            .map_err(|e| Error::structure(e.to_string()))?;
        inner
            .add_page_contents(page_id, bytes)
            .map_err(|e| Error::structure(e.to_string()))?;
    }
    Document::from_lopdf(inner, None)
}

/// How to install or supply OCR on this OS (`InvalidKind::OcrUnavailable`).
pub fn ocr_install_hint() -> String {
    if cfg!(windows) {
        "install Tesseract (winget install UB-Mannheim.TesseractOCR), set OPENPDF_TESSERACT, or pass a Windows.Media.Ocr OcrEngine"
            .into()
    } else if cfg!(target_os = "macos") {
        "install Tesseract (brew install tesseract), set OPENPDF_TESSERACT, or pass a Vision OcrEngine"
            .into()
    } else {
        "install tesseract-ocr or set OPENPDF_TESSERACT to the binary or libtesseract".into()
    }
}

fn language_install_hint(lang: &str) -> String {
    if cfg!(windows) {
        format!(
            "add the {lang} traineddata next to TESSDATA_PREFIX or the Tesseract installer language pack"
        )
    } else if cfg!(target_os = "macos") {
        "Homebrew: brew install tesseract tesseract-lang".into()
    } else {
        let pkg = match lang {
            "eng" => "tesseract-ocr".to_string(),
            other => format!("tesseract-ocr-{other}"),
        };
        format!("Debian/Ubuntu: apt install {pkg}")
    }
}

fn ocr_unavailable() -> Error {
    Error::invalid(InvalidKind::OcrUnavailable {
        hint: ocr_install_hint(),
    })
}

enum TessEngine {
    Binary(PathBuf),
    Library {
        lib: PathBuf,
        tessdata: Option<PathBuf>,
    },
}

impl TessEngine {
    fn backend(&self) -> OcrBackend {
        match self {
            TessEngine::Binary(_) => OcrBackend::Binary,
            TessEngine::Library { .. } => OcrBackend::Library,
        }
    }

    fn list_languages(&self) -> Vec<String> {
        match self {
            TessEngine::Binary(bin) => list_langs_binary(bin),
            TessEngine::Library { tessdata, .. } => tessdata
                .as_deref()
                .map(scan_tessdata)
                .unwrap_or_else(scan_default_tessdata),
        }
    }

    fn recognize(&self, raster: &Raster, lang: &str) -> Result<Vec<OcrWord>> {
        match self {
            TessEngine::Binary(bin) => recognize_binary(bin, raster, lang),
            TessEngine::Library { lib, tessdata } => {
                recognize_library(lib, tessdata.as_deref(), raster, lang)
            }
        }
    }
}

fn tesseract_bin_name() -> &'static str {
    if cfg!(windows) {
        "tesseract.exe"
    } else {
        "tesseract"
    }
}

fn tesseract_lib_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "libtesseract-5.dll",
            "libtesseract-4.dll",
            "tesseract54.dll",
            "tesseract.dll",
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "libtesseract.dylib",
            "libtesseract.5.dylib",
            "libtesseract.4.dylib",
        ]
    } else {
        &["libtesseract.so.5", "libtesseract.so.4", "libtesseract.so"]
    }
}

fn resolve_engine() -> Option<TessEngine> {
    if let Ok(raw) = std::env::var("OPENPDF_TESSERACT") {
        if let Some(found) = probe_path(Path::new(&raw)) {
            return Some(found);
        }
    }
    for candidate in tesseract_bin_candidates() {
        if is_executable(&candidate) {
            return Some(TessEngine::Binary(candidate));
        }
    }
    if let Some(bin) = which(tesseract_bin_name()).or_else(|| which("tesseract")) {
        return Some(TessEngine::Binary(bin));
    }
    for root in tesseract_lib_roots() {
        if let Some(found) = probe_path(&root) {
            return Some(found);
        }
    }
    for name in tesseract_lib_names() {
        if let Ok(lib) = unsafe { libloading::Library::new(name) } {
            drop(lib);
            return Some(TessEngine::Library {
                lib: PathBuf::from(name),
                tessdata: first_tessdata(),
            });
        }
    }
    None
}

fn tesseract_bin_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    {
        out.push(PathBuf::from("/usr/bin/tesseract"));
        out.push(PathBuf::from("/usr/local/bin/tesseract"));
        out.push(PathBuf::from("/app/bin/tesseract"));
    }
    #[cfg(target_os = "macos")]
    {
        out.push(PathBuf::from("/opt/homebrew/bin/tesseract"));
        out.push(PathBuf::from("/usr/local/bin/tesseract"));
    }
    #[cfg(windows)]
    {
        if let Ok(pf) = std::env::var("PROGRAMFILES") {
            out.push(
                PathBuf::from(pf)
                    .join("Tesseract-OCR")
                    .join("tesseract.exe"),
            );
        }
        if let Ok(pf86) = std::env::var("PROGRAMFILES(X86)") {
            out.push(
                PathBuf::from(pf86)
                    .join("Tesseract-OCR")
                    .join("tesseract.exe"),
            );
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            out.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Tesseract-OCR")
                    .join("tesseract.exe"),
            );
        }
    }
    out
}

fn tesseract_lib_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    {
        out.push(PathBuf::from("/app/extensions"));
        out.push(PathBuf::from("/usr/lib/x86_64-linux-gnu"));
        out.push(PathBuf::from("/usr/lib"));
        out.push(PathBuf::from("/usr/local/lib"));
    }
    #[cfg(target_os = "macos")]
    {
        out.push(PathBuf::from("/opt/homebrew/lib"));
        out.push(PathBuf::from("/usr/local/lib"));
    }
    #[cfg(windows)]
    {
        if let Ok(pf) = std::env::var("PROGRAMFILES") {
            out.push(PathBuf::from(pf).join("Tesseract-OCR"));
        }
    }
    out
}

fn probe_path(path: &Path) -> Option<TessEngine> {
    if path.is_file() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_lib = name.contains("tesseract")
            && path
                .extension()
                .is_some_and(|e| e == "so" || e == "dll" || e == "dylib" || name.contains(".so."));
        if is_lib || name.contains("libtesseract") {
            return Some(TessEngine::Library {
                lib: path.to_path_buf(),
                tessdata: first_tessdata(),
            });
        }
        if is_executable(path) {
            return Some(TessEngine::Binary(path.to_path_buf()));
        }
    }
    if path.is_dir() {
        let bin = path.join(tesseract_bin_name());
        if is_executable(&bin) {
            return Some(TessEngine::Binary(bin));
        }
        let plain = path.join("tesseract");
        if plain != bin && is_executable(&plain) {
            return Some(TessEngine::Binary(plain));
        }
        for name in tesseract_lib_names() {
            let lib = path.join(name);
            if lib.is_file() {
                return Some(TessEngine::Library {
                    lib,
                    tessdata: first_tessdata(),
                });
            }
        }
        if let Ok(rd) = std::fs::read_dir(path) {
            for ent in rd.flatten().take(64) {
                if let Some(found) = probe_path(&ent.path()) {
                    return Some(found);
                }
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    path.is_file()
        && Command::new(path)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(name);
        if is_executable(&p) {
            return Some(p);
        }
    }
    None
}

fn first_tessdata() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TESSDATA_PREFIX") {
        let p = PathBuf::from(p);
        if p.join("eng.traineddata").is_file() {
            return Some(p);
        }
        let nested = p.join("tessdata");
        if nested.join("eng.traineddata").is_file() {
            return Some(nested);
        }
    }
    scan_default_tessdata_dir()
}

fn scan_default_tessdata_dir() -> Option<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "linux")]
    {
        dirs.extend([
            PathBuf::from("/usr/share/tesseract-ocr/5/tessdata"),
            PathBuf::from("/usr/share/tesseract-ocr/4.00/tessdata"),
            PathBuf::from("/usr/share/tessdata"),
            PathBuf::from("/app/share/tessdata"),
        ]);
    }
    #[cfg(target_os = "macos")]
    {
        dirs.extend([
            PathBuf::from("/opt/homebrew/share/tessdata"),
            PathBuf::from("/usr/local/share/tessdata"),
        ]);
    }
    #[cfg(windows)]
    {
        if let Ok(pf) = std::env::var("PROGRAMFILES") {
            dirs.push(PathBuf::from(pf).join("Tesseract-OCR").join("tessdata"));
        }
    }
    dirs.into_iter().find(|d| d.is_dir())
}

fn scan_default_tessdata() -> Vec<String> {
    scan_default_tessdata_dir()
        .map(|d| scan_tessdata(&d))
        .unwrap_or_default()
}

fn scan_tessdata(dir: &Path) -> Vec<String> {
    let mut langs = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return langs;
    };
    for ent in rd.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if let Some(stem) = name.strip_suffix(".traineddata") {
            langs.push(stem.to_string());
        }
    }
    langs.sort();
    langs
}

fn list_langs_binary(bin: &Path) -> Vec<String> {
    let out = Command::new(bin).arg("--list-langs").output().ok();
    let Some(out) = out else {
        return scan_default_tessdata();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    let mut langs = Vec::new();
    for line in text.lines().chain(err.lines()) {
        let line = line.trim();
        if line.is_empty() || line.contains(' ') || line.contains(':') {
            continue;
        }
        langs.push(line.to_string());
    }
    if langs.is_empty() {
        return scan_default_tessdata();
    }
    langs
}

fn recognize_binary(bin: &Path, raster: &Raster, lang: &str) -> Result<Vec<OcrWord>> {
    let dir = std::env::temp_dir().join(format!(
        "openpdf-ocr-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir)?;
    let png = dir.join("page.png");
    let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
        raster.width,
        raster.height,
        raster.rgba.clone(),
    )
    .ok_or_else(|| Error::structure("RGBA buffer size"))?;
    img.save(&png)
        .map_err(|e| Error::structure(e.to_string()))?;
    let out = Command::new(bin)
        .arg(&png)
        .arg("stdout")
        .arg("-l")
        .arg(lang)
        .arg("tsv")
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let out = out.map_err(|_| ocr_unavailable())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("Error opening data file") || err.contains("Failed loading language") {
            return Err(Error::invalid(InvalidKind::OcrLanguageMissing {
                language: lang.to_string(),
                installed: list_langs_binary(bin).join(", "),
                hint: language_install_hint(lang),
            }));
        }
        return Err(Error::structure(err.into_owned()));
    }
    Ok(parse_tsv(&String::from_utf8_lossy(&out.stdout)))
}

fn recognize_library(
    lib_path: &Path,
    tessdata: Option<&Path>,
    raster: &Raster,
    lang: &str,
) -> Result<Vec<OcrWord>> {
    let rgb = rgba_to_rgb(raster);
    let lib = unsafe { libloading::Library::new(lib_path) }.map_err(|_| ocr_unavailable())?;
    unsafe {
        let create: libloading::Symbol<unsafe extern "C" fn() -> *mut c_void> = lib
            .get(b"TessBaseAPICreate\0")
            .map_err(|_| ocr_unavailable())?;
        let init3: libloading::Symbol<
            unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> i32,
        > = lib
            .get(b"TessBaseAPIInit3\0")
            .map_err(|_| ocr_unavailable())?;
        let set_image: libloading::Symbol<
            unsafe extern "C" fn(*mut c_void, *const u8, i32, i32, i32, i32),
        > = lib
            .get(b"TessBaseAPISetImage\0")
            .map_err(|_| ocr_unavailable())?;
        let get_tsv: libloading::Symbol<unsafe extern "C" fn(*mut c_void, i32) -> *mut c_char> =
            lib.get(b"TessBaseAPIGetTSVText\0")
                .map_err(|_| ocr_unavailable())?;
        let end: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> = lib
            .get(b"TessBaseAPIEnd\0")
            .map_err(|_| ocr_unavailable())?;
        let delete: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> = lib
            .get(b"TessBaseAPIDelete\0")
            .map_err(|_| ocr_unavailable())?;
        let delete_text: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> = lib
            .get(b"TessDeleteText\0")
            .map_err(|_| ocr_unavailable())?;

        let api = create();
        if api.is_null() {
            return Err(ocr_unavailable());
        }
        let lang_c = CString::new(lang).map_err(|e| Error::structure(e.to_string()))?;
        let data_c = tessdata.and_then(|p| CString::new(p.to_string_lossy().as_bytes()).ok());
        let datapath = data_c
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());
        let rc = init3(api, datapath, lang_c.as_ptr());
        if rc != 0 {
            end(api);
            delete(api);
            return Err(Error::invalid(InvalidKind::OcrLanguageMissing {
                language: lang.to_string(),
                installed: tessdata.map(scan_tessdata).unwrap_or_default().join(", "),
                hint: language_install_hint(lang),
            }));
        }
        set_image(
            api,
            rgb.as_ptr(),
            raster.width as i32,
            raster.height as i32,
            3,
            (raster.width * 3) as i32,
        );
        let ptr = get_tsv(api, 0);
        let words = if ptr.is_null() {
            Vec::new()
        } else {
            let tsv = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            delete_text(ptr);
            parse_tsv(&tsv)
        };
        end(api);
        delete(api);
        Ok(words)
    }
}

fn rgba_to_rgb(raster: &Raster) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((raster.width * raster.height * 3) as usize);
    for px in raster.rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&px[..3]);
    }
    rgb
}

fn parse_tsv(tsv: &str) -> Vec<OcrWord> {
    let mut words = Vec::new();
    for (i, line) in tsv.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        let level: i32 = cols[0].parse().unwrap_or(0);
        if level != 5 {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }
        let conf: f32 = cols[10].parse().unwrap_or(-1.0);
        if (0.0..20.0).contains(&conf) {
            continue;
        }
        let left: f32 = cols[6].parse().unwrap_or(0.0);
        let top: f32 = cols[7].parse().unwrap_or(0.0);
        let width: f32 = cols[8].parse().unwrap_or(0.0);
        let height: f32 = cols[9].parse().unwrap_or(0.0);
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        words.push(OcrWord {
            text: text.to_string(),
            left,
            top,
            width,
            height,
        });
    }
    words
}

fn invisible_text_ops(words: &[OcrWord], iw: u32, ih: u32, pw: f32, ph: f32) -> Vec<Operation> {
    let sx = pw / iw.max(1) as f32;
    let sy = ph / ih.max(1) as f32;
    let mut ops = vec![
        Operation::new("q", vec![]),
        Operation::new("BT", vec![]),
        Operation::new("Tr", vec![3.into()]),
    ];
    for w in words {
        let x = w.left * sx;
        let y = ph - (w.top + w.height) * sy;
        let size = (w.height * sy).clamp(4.0, 72.0);
        let box_w = w.width * sx;
        let bytes = winansi(&w.text);
        if bytes.is_empty() {
            continue;
        }
        let natural = helvetica_width_pt(&bytes, size);
        let tz = if natural > 0.01 {
            (box_w / natural * 100.0).clamp(1.0, 10_000.0)
        } else {
            100.0
        };
        ops.push(Operation::new("Tf", vec!["Helv".into(), size.into()]));
        ops.push(Operation::new("Tz", vec![tz.into()]));
        ops.push(Operation::new("Td", vec![x.into(), y.into()]));
        ops.push(Operation::new(
            "Tj",
            vec![Object::String(bytes, StringFormat::Literal)],
        ));
        ops.push(Operation::new("Td", vec![(-x).into(), (-y).into()]));
    }
    ops.push(Operation::new("ET", vec![]));
    ops.push(Operation::new("Q", vec![]));
    ops
}

/// Helvetica WinAnsi advances in 1/1000 em (Adobe AFM), ASCII only; else 600.
fn helvetica_advance(b: u8) -> u16 {
    const ASCII: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
        500, 334, 260, 334, 584,
    ];
    match b {
        32..=126 => ASCII[(b - 32) as usize],
        _ => 600,
    }
}

fn helvetica_width_pt(bytes: &[u8], size: f32) -> f32 {
    let units: u32 = bytes.iter().map(|&b| u32::from(helvetica_advance(b))).sum();
    units as f32 * size / 1000.0
}

fn winansi(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
        .collect()
}

fn helvetica_winansi(doc: &mut lopdf::Document) -> ObjectId {
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"Font".to_vec()));
    dict.set("Subtype", Object::Name(b"Type1".to_vec()));
    dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    dict.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    doc.add_object(Object::Dictionary(dict))
}

fn ensure_font(doc: &mut lopdf::Document, page_id: ObjectId, font_id: ObjectId) -> Result<()> {
    let page = doc
        .get_object_mut(page_id)
        .map_err(|e| Error::structure(e.to_string()))?;
    let dict = page
        .as_dict_mut()
        .map_err(|e| Error::structure(e.to_string()))?;
    let mut resources = match dict.get(b"Resources") {
        Ok(Object::Dictionary(d)) => d.clone(),
        Ok(Object::Reference(id)) => {
            let id = *id;
            return ensure_font_in_resources(doc, id, font_id);
        }
        _ => Dictionary::new(),
    };
    let mut fonts = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    fonts.set("Helv", Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));
    dict.set("Resources", Object::Dictionary(resources));
    Ok(())
}

fn ensure_font_in_resources(
    doc: &mut lopdf::Document,
    resources_id: ObjectId,
    font_id: ObjectId,
) -> Result<()> {
    let obj = doc
        .get_object_mut(resources_id)
        .map_err(|e| Error::structure(e.to_string()))?;
    let resources = obj
        .as_dict_mut()
        .map_err(|e| Error::structure(e.to_string()))?;
    let mut fonts = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    fonts.set("Helv", Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));
    Ok(())
}

fn page_size(doc: &lopdf::Document, mut id: ObjectId) -> Option<(f32, f32)> {
    for _ in 0..16 {
        let dict = doc.get_object(id).ok()?.as_dict().ok()?;
        if let Ok(mb) = dict.get(b"MediaBox") {
            let arr = mb.as_array().ok()?;
            if arr.len() < 4 {
                return None;
            }
            let n = |i: usize| match arr[i] {
                Object::Integer(v) => Some(v as f32),
                Object::Real(v) => Some(v),
                _ => None,
            };
            return Some(((n(2)? - n(0)?).abs(), (n(3)? - n(1)?).abs()));
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::append_stub_signature;
    use crate::test_support::corpus;

    struct FakeOcr;

    impl OcrEngine for FakeOcr {
        fn capability(&self) -> OcrCapability {
            OcrCapability {
                available: true,
                backend: Some(OcrBackend::Platform),
                languages: vec!["eng".into()],
            }
        }

        fn languages(&self) -> Vec<String> {
            vec!["eng".into()]
        }

        fn recognize(&self, _raster: &Raster, _language: &str) -> Result<Vec<OcrWord>> {
            Ok(vec![OcrWord {
                text: "Hello".into(),
                left: 10.0,
                top: 20.0,
                width: 40.0,
                height: 12.0,
            }])
        }
    }

    #[test]
    fn cargo_toml_does_not_link_tesseract() {
        let toml = include_str!("../Cargo.toml");
        assert!(
            !toml.to_ascii_lowercase().contains("tesseract"),
            "Tesseract must stay a runtime probe"
        );
        assert!(!toml.to_ascii_lowercase().contains("leptonica"));
    }

    #[test]
    fn capability_query_never_panics() {
        let cap = ocr_capability();
        if !cap.available {
            assert!(cap.backend.is_none());
            let err = Error::invalid(InvalidKind::OcrUnavailable {
                hint: ocr_install_hint(),
            });
            assert!(matches!(
                err,
                Error::Invalid(InvalidKind::OcrUnavailable { .. })
            ));
        }
    }

    #[test]
    fn unavailable_engine_carries_hint() {
        let eng = UnavailableOcr::new(ocr_install_hint());
        assert!(!eng.capability().available);
        let err = eng
            .recognize(
                &Raster {
                    width: 1,
                    height: 1,
                    rgba: vec![0, 0, 0, 255],
                },
                "eng",
            )
            .unwrap_err();
        match err {
            Error::Invalid(InvalidKind::OcrUnavailable { hint }) => {
                assert!(!hint.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ocr_refuses_signed() {
        let Some(engine) = Engine::new().ok() else {
            if std::env::var("CI").is_ok() {
                panic!("PDFium required in CI");
            }
            return;
        };
        let bytes = append_stub_signature(&std::fs::read(corpus("hello.pdf")).unwrap()).unwrap();
        let signed = Document::from_bytes(bytes, None).unwrap();
        assert!(matches!(
            ocr(&engine, &FakeOcr, &signed, &OcrOptions::default()),
            Err(Error::Signed { op: SignedOp::Edit })
        ));
    }

    #[test]
    fn ocr_writes_invisible_layer_from_engine() {
        let Some(engine) = Engine::new().ok() else {
            if std::env::var("CI").is_ok() {
                panic!("PDFium required in CI");
            }
            return;
        };
        let doc = engine.open(corpus("hello.pdf")).unwrap();
        let out = ocr(&engine, &FakeOcr, &doc, &OcrOptions::default()).unwrap();
        assert_eq!(out.page_count(), 1);
        let inner = out.assembled_lopdf().unwrap();
        let id = *inner.get_pages().get(&1).unwrap();
        let bytes = inner.get_page_content(id).unwrap();
        let decoded = Content::decode(&bytes).unwrap();
        assert!(
            decoded.operations.iter().any(|op| op.operator == "Tr"
                && op.operands.first().and_then(|o| o.as_i64().ok()) == Some(3)),
            "expected invisible text rendering mode"
        );
    }

    #[test]
    fn ocr_adds_text_layer_when_tesseract_present() {
        let cap = ocr_capability();
        if !cap.available {
            if std::env::var("CI").is_ok() && cfg!(target_os = "linux") {
                panic!("tesseract-ocr is required in Linux CI");
            }
            return;
        }
        let engine = Engine::new().expect("pdfium");
        let doc = engine.open(corpus("hello.pdf")).unwrap();
        let tess = default_ocr_engine();
        let out = ocr(&engine, tess.as_ref(), &doc, &OcrOptions::default()).unwrap();
        assert_eq!(out.page_count(), 1);
        let inner = out.assembled_lopdf().unwrap();
        let id = *inner.get_pages().get(&1).unwrap();
        let bytes = inner.get_page_content(id).unwrap();
        let decoded = Content::decode(&bytes).unwrap();
        assert!(
            decoded.operations.iter().any(|op| op.operator == "Tr"
                && op.operands.first().and_then(|o| o.as_i64().ok()) == Some(3)),
            "expected invisible text rendering mode"
        );
    }

    #[test]
    fn parse_tsv_reads_words() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
5\t1\t1\t1\t1\t1\t10\t20\t40\t12\t90\tHello\n";
        let words = parse_tsv(tsv);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "Hello");
        assert_eq!(words[0].width, 40.0);
        assert_eq!(words[0].height, 12.0);
    }

    #[test]
    fn invisible_text_scales_horizontally_to_word_width() {
        let words = [OcrWord {
            text: "Hello".into(),
            left: 10.0,
            top: 20.0,
            width: 40.0,
            height: 12.0,
        }];
        let ops = invisible_text_ops(&words, 100, 100, 100.0, 100.0);
        let tz = ops
            .iter()
            .find(|op| op.operator == "Tz")
            .expect("Tz so selection boxes match word width");
        let value = match tz.operands.first() {
            Some(Object::Real(v)) => *v,
            Some(Object::Integer(v)) => *v as f32,
            other => panic!("Tz operand {other:?}"),
        };
        let natural = helvetica_width_pt(&winansi("Hello"), 12.0);
        let expected = (40.0 / natural * 100.0).clamp(1.0, 10_000.0);
        assert!(
            (value - expected).abs() < 0.05,
            "tz={value} expected={expected}"
        );
    }
}
