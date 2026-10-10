//! macOS Vision (`VNRecognizeTextRequest`). Boxes are converted from
//! normalized bottom-left to page top-left pixels.

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_foundation::{CFData, CGRect};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequestTextRecognitionLevel,
};
use openpdf_core::{tr, OcrBackend, OcrCapability, OcrEngine, OcrError, OcrWord};

use crate::geom::{pack_rgba, vision_box_to_pixels};
use crate::lang::{language_accepted, to_vision_tag};

/// Native Vision OCR. Unit struct so the type stays `Send + Sync`.
#[derive(Clone, Debug, Default)]
pub struct VisionOcr;

impl VisionOcr {
    pub fn probe() -> Option<Self> {
        if listed_languages().is_empty() {
            None
        } else {
            Some(Self)
        }
    }
}

fn hint() -> String {
    tr("install the Vision OCR language pack in System Settings")
}

fn listed_languages() -> Vec<String> {
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    let langs = unsafe {
        request
            .supportedRecognitionLanguagesAndReturnError()
            .unwrap_or_else(|_| request.recognitionLanguages())
    };
    langs.iter().map(|s| s.to_string()).collect()
}

fn rgba_to_cgimage(
    width: u32,
    height: u32,
    stride: u32,
    rgba: &[u8],
) -> std::result::Result<objc2_core_foundation::CFRetained<CGImage>, OcrError> {
    let packed = pack_rgba(width, height, stride, rgba);
    let data = NSData::from_vec(packed);
    let cf: &CFData = data.as_ref();
    let provider = CGDataProvider::with_cf_data(Some(cf)).ok_or_else(|| OcrError::Failed {
        message: "CGDataProvider".into(),
    })?;
    let space = CGColorSpace::new_device_rgb().ok_or_else(|| OcrError::Failed {
        message: "CGColorSpace".into(),
    })?;
    let info = CGBitmapInfo(CGImageAlphaInfo::Last.0);
    unsafe {
        CGImage::new(
            width as usize,
            height as usize,
            8,
            32,
            (width as usize) * 4,
            Some(&space),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| OcrError::Failed {
        message: "CGImage".into(),
    })
}

impl OcrEngine for VisionOcr {
    fn capability(&self) -> OcrCapability {
        let languages = listed_languages();
        if languages.is_empty() {
            return OcrCapability::unavailable();
        }
        OcrCapability {
            available: true,
            backend: Some(OcrBackend::Platform),
            languages,
        }
    }

    fn languages(&self) -> Vec<String> {
        listed_languages()
    }

    fn recognize(
        &self,
        width: u32,
        height: u32,
        stride: u32,
        rgba: Vec<u8>,
        languages: Vec<String>,
    ) -> std::result::Result<Vec<OcrWord>, OcrError> {
        let installed = listed_languages();
        if installed.is_empty() {
            return Err(OcrError::Unavailable { hint: hint() });
        }
        if !languages.is_empty() && !languages.iter().any(|l| language_accepted(l, &installed)) {
            return Err(OcrError::LanguageMissing {
                language: languages.join(", "),
                installed: installed.join(", "),
                hint: hint(),
            });
        }
        let mapped: Vec<String> = if languages.is_empty() {
            installed.clone()
        } else {
            languages.iter().map(|t| to_vision_tag(t)).collect()
        };
        let ns_langs: Vec<Retained<NSString>> =
            mapped.iter().map(|t| NSString::from_str(t)).collect();
        let lang_array = NSArray::from_retained_slice(&ns_langs);

        let image = rgba_to_cgimage(width, height, stride, &rgba)?;
        let options: Retained<NSDictionary<VNImageOption, objc2::runtime::AnyObject>> =
            NSDictionary::new();
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                &image,
                &options,
            )
        };
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setRecognitionLanguages(&lang_array);

        let requests = NSArray::from_slice(&[request.as_ref()]);
        handler
            .performRequests_error(&requests)
            .map_err(|e| OcrError::Failed {
                message: e.to_string(),
            })?;

        let mut words = Vec::new();
        let Some(results) = request.results() else {
            return Ok(words);
        };
        for obs in results.iter() {
            let box_: CGRect = unsafe { obs.boundingBox() };
            let (x, y, w, h) = vision_box_to_pixels(
                box_.origin.x as f32,
                box_.origin.y as f32,
                box_.size.width as f32,
                box_.size.height as f32,
                width as f32,
                height as f32,
            );
            let candidates = obs.topCandidates(1);
            let Some(first) = candidates.iter().next() else {
                continue;
            };
            let text = first.string().to_string();
            let confidence = first.confidence() as f32;
            if text.is_empty() {
                continue;
            }
            words.push(OcrWord {
                text,
                x,
                y,
                w,
                h,
                confidence,
            });
        }
        Ok(words)
    }
}
