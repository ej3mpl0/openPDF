//! Map BCP-47 tags (`en`, `es-ES`) to each engine's language form.

/// Lowercase BCP-47 with `-` separators.
pub fn normalize_bcp47(tag: &str) -> String {
    tag.trim().replace('_', "-").to_ascii_lowercase()
}

/// Primary subtag (`en-US` -> `en`, `eng` -> `en`).
pub fn primary_language(tag: &str) -> String {
    let n = normalize_bcp47(tag);
    match n.as_str() {
        "eng" => "en".into(),
        "spa" => "es".into(),
        "fra" | "fre" => "fr".into(),
        "deu" | "ger" => "de".into(),
        "por" => "pt".into(),
        "ita" => "it".into(),
        other => other.split('-').next().unwrap_or(other).to_string(),
    }
}

/// Windows.Media.Ocr / `Language` tag (`en`, `en-US`, `es`).
pub fn to_windows_tag(tag: &str) -> String {
    let n = normalize_bcp47(tag);
    match n.as_str() {
        "eng" => "en".into(),
        "spa" => "es".into(),
        other => other.to_string(),
    }
}

/// Vision ISO tag (`en-US`, `es-ES`).
pub fn to_vision_tag(tag: &str) -> String {
    let n = normalize_bcp47(tag);
    match n.as_str() {
        "en" | "eng" | "en-us" => "en-US".into(),
        "es" | "spa" | "es-es" => "es-ES".into(),
        "fr" | "fra" | "fre" | "fr-fr" => "fr-FR".into(),
        "de" | "deu" | "ger" | "de-de" => "de-DE".into(),
        "pt" | "por" | "pt-br" => "pt-BR".into(),
        "it" | "ita" | "it-it" => "it-IT".into(),
        other if other.len() == 2 => {
            format!("{other}-{}", other.to_ascii_uppercase())
        }
        other => other.to_string(),
    }
}

/// Tesseract traineddata stem (`eng`, `spa`).
pub fn to_tesseract_tag(tag: &str) -> String {
    match primary_language(tag).as_str() {
        "en" => "eng".into(),
        "es" => "spa".into(),
        "fr" => "fra".into(),
        "de" => "deu".into(),
        "pt" => "por".into(),
        "it" => "ita".into(),
        other => other.to_string(),
    }
}

/// True when `requested` matches an installed tag (primary or full).
pub fn language_accepted(requested: &str, installed: &[String]) -> bool {
    let req = normalize_bcp47(requested);
    let req_primary = primary_language(&req);
    let req_tess = to_tesseract_tag(&req);
    let req_win = to_windows_tag(&req);
    let req_vis = normalize_bcp47(&to_vision_tag(&req));
    installed.iter().any(|inst| {
        let n = normalize_bcp47(inst);
        n == req
            || n == req_win
            || n == req_vis
            || primary_language(&n) == req_primary
            || to_tesseract_tag(&n) == req_tess
            || normalize_bcp47(&to_vision_tag(inst)) == req_vis
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_english_and_spanish() {
        assert_eq!(to_windows_tag("eng"), "en");
        assert_eq!(to_windows_tag("es-ES"), "es-es");
        assert_eq!(to_vision_tag("en"), "en-US");
        assert_eq!(to_vision_tag("spa"), "es-ES");
        assert_eq!(to_tesseract_tag("en-US"), "eng");
        assert_eq!(to_tesseract_tag("es"), "spa");
        assert_eq!(primary_language("en-GB"), "en");
    }

    #[test]
    fn accepts_related_tags() {
        assert!(language_accepted("en", &["en-US".into()]));
        assert!(language_accepted("eng", &["en".into()]));
        assert!(language_accepted("es-ES", &["spa".into()]));
        assert!(!language_accepted("ja", &["en".into()]));
    }
}
