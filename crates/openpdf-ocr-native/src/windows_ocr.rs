//! Windows.Media.Ocr. Word confidence is not provided by the OS; we store 1.0.

use openpdf_core::{tr, OcrBackend, OcrCapability, OcrEngine, OcrError, OcrWord, UnavailableOcr};
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine as WinOcr;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

use crate::geom::{pack_rgba, windows_rect_to_word};
use crate::lang::{language_accepted, to_windows_tag};

/// Native Windows OCR. Create per call so the type stays `Send + Sync`.
#[derive(Clone, Debug, Default)]
pub struct WindowsOcr;

impl WindowsOcr {
    pub fn probe() -> Option<Self> {
        if listed_languages().is_empty() {
            None
        } else {
            Some(Self)
        }
    }
}

fn listed_languages() -> Vec<String> {
    let Ok(list) = WinOcr::AvailableRecognizerLanguages() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let Ok(n) = list.Size() else {
        return out;
    };
    for i in 0..n {
        if let Ok(lang) = list.GetAt(i) {
            if let Ok(tag) = lang.LanguageTag() {
                out.push(tag.to_string());
            }
        }
    }
    out
}

fn hint() -> String {
    tr("install the Windows OCR language pack")
}

fn software_bitmap(
    width: u32,
    height: u32,
    stride: u32,
    rgba: &[u8],
) -> std::result::Result<SoftwareBitmap, OcrError> {
    let packed = pack_rgba(width, height, stride, rgba);
    let mut bgra = Vec::with_capacity(packed.len());
    for px in packed.chunks_exact(4) {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    let stream = InMemoryRandomAccessStream::new().map_err(|e| OcrError::Failed {
        message: e.to_string(),
    })?;
    let writer = DataWriter::CreateDataWriter(&stream).map_err(|e| OcrError::Failed {
        message: e.to_string(),
    })?;
    writer.WriteBytes(&bgra).map_err(|e| OcrError::Failed {
        message: e.to_string(),
    })?;
    writer
        .StoreAsync()
        .map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?
        .get()
        .map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?;
    writer
        .FlushAsync()
        .map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?
        .get()
        .map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?;
    let size = bgra.len() as u32;
    stream.Seek(0).map_err(|e| OcrError::Failed {
        message: e.to_string(),
    })?;
    let input = stream.GetInputStreamAt(0).map_err(|e| OcrError::Failed {
        message: e.to_string(),
    })?;
    let reader = windows::Storage::Streams::DataReader::CreateDataReader(&input).map_err(|e| {
        OcrError::Failed {
            message: e.to_string(),
        }
    })?;
    reader
        .LoadAsync(size)
        .map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?
        .get()
        .map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?;
    let buffer = reader.ReadBuffer(size).map_err(|e| OcrError::Failed {
        message: e.to_string(),
    })?;
    SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(|e| OcrError::Failed {
        message: e.to_string(),
    })
}

fn engine_for(tags: &[String]) -> std::result::Result<WinOcr, OcrError> {
    let installed = listed_languages();
    if installed.is_empty() {
        return Err(OcrError::Unavailable { hint: hint() });
    }
    for tag in tags {
        let mapped = to_windows_tag(tag);
        if !language_accepted(tag, &installed) {
            continue;
        }
        if let Ok(lang) = Language::CreateLanguage(&HSTRING::from(mapped)) {
            if let Ok(eng) = WinOcr::TryCreateFromLanguage(&lang) {
                return Ok(eng);
            }
        }
    }
    if tags.is_empty() {
        return WinOcr::TryCreateFromUserProfileLanguages().map_err(|e| OcrError::Failed {
            message: e.to_string(),
        });
    }
    Err(OcrError::LanguageMissing {
        language: tags.join(", "),
        installed: installed.join(", "),
        hint: hint(),
    })
}

impl OcrEngine for WindowsOcr {
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
        let engine = engine_for(&languages)?;
        let bitmap = software_bitmap(width, height, stride, &rgba)?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| OcrError::Failed {
                message: e.to_string(),
            })?
            .get()
            .map_err(|e| OcrError::Failed {
                message: e.to_string(),
            })?;
        let mut words = Vec::new();
        let lines = result.Lines().map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?;
        let n = lines.Size().map_err(|e| OcrError::Failed {
            message: e.to_string(),
        })?;
        for i in 0..n {
            let line = lines.GetAt(i).map_err(|e| OcrError::Failed {
                message: e.to_string(),
            })?;
            let wlist = line.Words().map_err(|e| OcrError::Failed {
                message: e.to_string(),
            })?;
            let wn = wlist.Size().map_err(|e| OcrError::Failed {
                message: e.to_string(),
            })?;
            for j in 0..wn {
                let word = wlist.GetAt(j).map_err(|e| OcrError::Failed {
                    message: e.to_string(),
                })?;
                let text = word
                    .Text()
                    .map_err(|e| OcrError::Failed {
                        message: e.to_string(),
                    })?
                    .to_string();
                let rect = word.BoundingRect().map_err(|e| OcrError::Failed {
                    message: e.to_string(),
                })?;
                words.push(windows_rect_to_word(
                    text,
                    rect.X as f32,
                    rect.Y as f32,
                    rect.Width as f32,
                    rect.Height as f32,
                ));
            }
        }
        Ok(words)
    }
}

#[allow(dead_code)]
fn _unavailable_fallback() -> UnavailableOcr {
    UnavailableOcr::new(hint())
}
