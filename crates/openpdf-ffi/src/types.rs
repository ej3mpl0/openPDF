use openpdf_core::{
    CertificateInfo, CertificateUsage, OutlineItem as CoreOutline, PageInfo as CorePage,
    PageRange as CorePageRange, PdfPermissions, Raster as CoreRaster, SearchHit as CoreHit, Tile,
};

use crate::error::FfiError;

/// Page metrics in PDF points (1/72 in). `index` is 0-based.
#[derive(Clone, Debug, uniffi::Record)]
pub struct PageInfo {
    pub index: u32,
    pub width_pt: f32,
    pub height_pt: f32,
    pub rotation: i32,
}

impl From<CorePage> for PageInfo {
    fn from(p: CorePage) -> Self {
        Self {
            index: p.index,
            width_pt: p.width_pt,
            height_pt: p.height_pt,
            rotation: p.rotation,
        }
    }
}

/// Inclusive 0-based page range.
#[derive(Clone, Debug, uniffi::Record)]
pub struct PageRange {
    pub start: u32,
    pub end: u32,
}

impl PageRange {
    pub(crate) fn to_core(&self) -> openpdf_core::Result<CorePageRange> {
        CorePageRange::new(self.start, self.end)
    }
}

/// Bookmark entry. `page` is 0-based when it resolves.
#[derive(Clone, Debug, uniffi::Record)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u32>,
    pub children: Vec<OutlineItem>,
}

impl From<CoreOutline> for OutlineItem {
    fn from(item: CoreOutline) -> Self {
        Self {
            title: item.title,
            page: item.page,
            children: item.children.into_iter().map(Self::from).collect(),
        }
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct SearchHit {
    pub page: u32,
    pub start: u64,
    pub end: u64,
    pub context: String,
}

impl From<CoreHit> for SearchHit {
    fn from(hit: CoreHit) -> Self {
        Self {
            page: hit.page,
            start: hit.start as u64,
            end: hit.end as u64,
            context: hit.context,
        }
    }
}

/// One rendered tile (unpremultiplied RGBA8, row-major, no padding).
#[derive(Clone, Debug, uniffi::Record)]
pub struct RasterTile {
    pub page: u32,
    pub zoom: f32,
    pub tx: u32,
    pub ty: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RasterTile {
    pub(crate) fn from_tile(tile: Tile) -> Self {
        Self {
            page: tile.key.page,
            zoom: tile.key.zoom(),
            tx: tile.key.x,
            ty: tile.key.y,
            width: tile.width,
            height: tile.height,
            rgba: tile.rgba,
        }
    }
}

/// Full-page raster (unpremultiplied RGBA8).
#[derive(Clone, Debug, uniffi::Record)]
pub struct PageRaster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl From<CoreRaster> for PageRaster {
    fn from(r: CoreRaster) -> Self {
        Self {
            width: r.width,
            height: r.height,
            rgba: r.rgba,
        }
    }
}

/// AES-256 user-facing permission flags.
#[derive(Clone, Debug, uniffi::Record)]
pub struct Permissions {
    pub print: bool,
    pub modify: bool,
    pub copy: bool,
    pub annotate: bool,
    pub fill: bool,
    pub assemble: bool,
    pub print_high: bool,
}

impl Permissions {
    pub(crate) fn to_core(&self) -> PdfPermissions {
        PdfPermissions {
            print: self.print,
            modify: self.modify,
            copy: self.copy,
            annotate: self.annotate,
            fill: self.fill,
            assemble: self.assemble,
            print_high: self.print_high,
        }
    }
}

/// Token certificate. `id` is what [`crate::PdfDocument::sign`] must carry.
#[derive(Clone, Debug, uniffi::Record)]
pub struct Certificate {
    pub id: String,
    pub subject: String,
    pub issuer: String,
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    pub usages: Vec<String>,
}

impl From<CertificateInfo> for Certificate {
    fn from(info: CertificateInfo) -> Self {
        Self {
            id: info.id,
            subject: info.subject,
            issuer: info.issuer,
            not_before: info.validity.not_before,
            not_after: info.validity.not_after,
            usages: info.usage.iter().map(usage_name).collect(),
        }
    }
}

fn usage_name(usage: &CertificateUsage) -> String {
    match usage {
        CertificateUsage::Signing => "signing".into(),
        CertificateUsage::Authentication => "authentication".into(),
        CertificateUsage::Encryption => "encryption".into(),
        CertificateUsage::Other(detail) => detail.clone(),
    }
}

pub(crate) fn convert_ranges(ranges: &[PageRange]) -> Result<Vec<CorePageRange>, FfiError> {
    ranges
        .iter()
        .map(|r| r.to_core().map_err(FfiError::from_core))
        .collect()
}
