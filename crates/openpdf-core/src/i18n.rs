//! Gettext-compatible catalogs (`.po`) shared with the GTK UI.
//!
//! English lives in source as `msgid`. Translations live under `po/{lang}.po`.
//! Locale follows GNU gettext: `LANGUAGE` (colon list) first, then `LC_ALL` /
//! `LC_MESSAGES` / `LANG`. `LANGUAGE` is ignored when that LC locale is `C` or
//! `POSIX`. English is the fallback.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

include!(concat!(env!("OUT_DIR"), "/embedded_po.rs"));

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    entries: HashMap<String, String>,
}

impl Catalog {
    pub fn empty() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn parse(src: &str) -> Self {
        let mut entries = HashMap::new();
        let mut msgid = String::new();
        let mut msgstr = String::new();
        let mut field = Field::None;

        for raw in src.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                flush(&mut entries, &mut msgid, &mut msgstr, &mut field);
                continue;
            }
            if let Some(rest) = line.strip_prefix("msgid ") {
                flush(&mut entries, &mut msgid, &mut msgstr, &mut field);
                field = Field::Id;
                msgid.push_str(&unquote(rest));
            } else if let Some(rest) = line.strip_prefix("msgstr ") {
                field = Field::Str;
                msgstr.push_str(&unquote(rest));
            } else if line.starts_with("msgctxt ") {
                flush(&mut entries, &mut msgid, &mut msgstr, &mut field);
                field = Field::None;
            } else if line.starts_with('"') {
                let chunk = unquote(line);
                match field {
                    Field::Id => msgid.push_str(&chunk),
                    Field::Str => msgstr.push_str(&chunk),
                    Field::None => {}
                }
            }
        }
        flush(&mut entries, &mut msgid, &mut msgstr, &mut field);
        entries.remove("");
        Self { entries }
    }

    pub fn translate<'a>(&'a self, msgid: &'a str) -> &'a str {
        self.entries
            .get(msgid)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(msgid)
    }
}

#[derive(Clone, Copy)]
enum Field {
    None,
    Id,
    Str,
}

fn flush(
    entries: &mut HashMap<String, String>,
    msgid: &mut String,
    msgstr: &mut String,
    field: &mut Field,
) {
    if !msgid.is_empty() && !msgstr.is_empty() {
        entries.insert(std::mem::take(msgid), std::mem::take(msgstr));
    } else {
        msgid.clear();
        msgstr.clear();
    }
    *field = Field::None;
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

struct State {
    locale: String,
    catalog: Catalog,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        locale: String::new(),
        catalog: Catalog::empty(),
    })
});

/// Read GNU gettext locale variables and load that catalog.
pub fn init() {
    set_locale(&detect_locale());
}

pub fn detect_locale() -> String {
    detect_locale_from(
        env::var("LANGUAGE").ok().as_deref(),
        env::var("LC_ALL").ok().as_deref(),
        env::var("LC_MESSAGES").ok().as_deref(),
        env::var("LANG").ok().as_deref(),
    )
}

fn detect_locale_from(
    language: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> String {
    let lc = [lc_all, lc_messages, lang]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|v| !v.is_empty());
    // Unset LC_* / LANG is C, so LANGUAGE must not apply.
    let c_locale = lc.map(is_c_or_posix).unwrap_or(true);

    if !c_locale {
        if let Some(list) = language {
            if let Some(tag) = first_available_language(list) {
                return tag;
            }
        }
    }
    if let Some(val) = lc {
        if let Some(tag) = first_language(val) {
            return tag;
        }
    }
    "en".into()
}

fn is_c_or_posix(val: &str) -> bool {
    let first = val
        .split(':')
        .map(str::trim)
        .find(|part| !part.is_empty())
        .unwrap_or("");
    if first.is_empty() {
        return false;
    }
    let tag = first.split('.').next().unwrap_or(first);
    let tag = tag.split('@').next().unwrap_or(tag);
    tag == "C" || tag == "POSIX"
}

fn language_tags(val: &str) -> Vec<String> {
    val.split(':').filter_map(normalize_tag).collect()
}

fn normalize_tag(part: &str) -> Option<String> {
    let part = part.trim();
    if part.is_empty() {
        return None;
    }
    let tag = part.split('.').next().unwrap_or(part);
    let tag = tag.split('@').next().unwrap_or(tag);
    if tag.is_empty() || tag == "C" || tag == "POSIX" {
        return None;
    }
    Some(tag.replace('-', "_"))
}

fn first_language(val: &str) -> Option<String> {
    language_tags(val).into_iter().next()
}

fn first_available_language(val: &str) -> Option<String> {
    language_tags(val)
        .into_iter()
        .find(|tag| catalog_available(tag))
}

fn catalog_available(tag: &str) -> bool {
    if is_english(tag) {
        return true;
    }
    locale_fallbacks(tag)
        .iter()
        .any(|code| read_po_file(code).is_some() || embedded_po(code).is_some())
}

/// Force a locale (`es`, `es_ES`, `en`, or a `LANGUAGE`-style list). Used by tests and the CLI.
pub fn set_locale(tag: &str) {
    let locale = first_available_language(tag)
        .or_else(|| first_language(tag))
        .unwrap_or_else(|| "en".into());
    let catalog = load_catalog(&locale);
    if let Ok(mut st) = STATE.lock() {
        st.locale = locale;
        st.catalog = catalog;
    }
}

pub fn current_locale() -> String {
    STATE
        .lock()
        .map(|s| {
            if s.locale.is_empty() {
                "en".into()
            } else {
                s.locale.clone()
            }
        })
        .unwrap_or_else(|_| "en".into())
}

fn ensure_init() {
    let need = STATE.lock().map(|s| s.locale.is_empty()).unwrap_or(false);
    if need {
        init();
    }
}

/// Translate `msgid`. Missing translations return the English msgid.
pub fn tr(msgid: &str) -> String {
    ensure_init();
    STATE
        .lock()
        .map(|s| s.catalog.translate(msgid).to_string())
        .unwrap_or_else(|_| msgid.to_string())
}

/// Translate `msgid` and replace `{name}` placeholders.
pub fn tr_args(msgid: &str, args: &[(&str, &str)]) -> String {
    let mut out = tr(msgid);
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

fn load_catalog(tag: &str) -> Catalog {
    if is_english(tag) {
        return Catalog::empty();
    }
    let candidates = locale_fallbacks(tag);
    for code in &candidates {
        if let Some(src) = read_po_file(code) {
            return Catalog::parse(&src);
        }
    }
    for code in &candidates {
        if let Some(src) = embedded_po(code) {
            return Catalog::parse(src);
        }
    }
    Catalog::empty()
}

fn is_english(tag: &str) -> bool {
    let t = tag.to_ascii_lowercase();
    t == "en" || t.starts_with("en_")
}

fn locale_fallbacks(tag: &str) -> Vec<String> {
    let mut out = vec![tag.to_string()];
    if let Some((lang, _)) = tag.split_once('_') {
        if !out.iter().any(|s| s == lang) {
            out.push(lang.to_string());
        }
    }
    out
}

fn embedded_po(code: &str) -> Option<&'static str> {
    EMBEDDED_PO
        .iter()
        .find(|(lang, _)| *lang == code)
        .map(|(_, src)| *src)
}

fn read_po_file(code: &str) -> Option<String> {
    for dir in po_search_dirs() {
        let path = dir.join(format!("{code}.po"));
        if let Ok(src) = fs::read_to_string(&path) {
            return Some(src);
        }
    }
    None
}

fn po_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(p) = env::var("OPENPDF_PO_DIR") {
        if !p.is_empty() {
            dirs.push(PathBuf::from(p));
        }
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("po"));
            dirs.push(dir.join("../po"));
        }
    }
    dirs.push(PathBuf::from("po"));
    if let Ok(manifest) = env::var("CARGO_MANIFEST_DIR") {
        dirs.push(Path::new(&manifest).join("../../po"));
        dirs.push(Path::new(&manifest).join("po"));
    }
    dirs
}

/// Language codes with an embedded or on-disk catalog (plus English).
pub fn available_locales() -> Vec<String> {
    let mut langs = vec!["en".to_string()];
    for (lang, _) in EMBEDDED_PO {
        if !langs.iter().any(|l| l == lang) {
            langs.push((*lang).to_string());
        }
    }
    for dir in po_search_dirs() {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for ent in rd.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if let Some(code) = name.strip_suffix(".po") {
                if code != "openpdf" && !langs.iter().any(|l| l == code) {
                    langs.push(code.to_string());
                }
            }
        }
    }
    langs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn parser_substitutes_msgstr() {
        let cat = Catalog::parse(
            r#"
msgid "Hello"
msgstr "HELLO"
"#,
        );
        assert_eq!(cat.translate("Hello"), "HELLO");
        assert_eq!(cat.translate("missing"), "missing");
    }

    #[test]
    fn parser_joins_wrapped_strings() {
        let cat = Catalog::parse("msgid \"\"\n\"Hel\"\n\"lo\"\nmsgstr \"\"\n\"HEL\"\n\"LO\"\n");
        assert_eq!(cat.translate("Hello"), "HELLO");
    }

    #[test]
    fn english_is_msgid_fallback() {
        crate::test_support::with_locale("en", || {
            assert_eq!(tr("document has no pages"), "document has no pages");
            assert_eq!(current_locale(), "en");
        });
    }

    #[test]
    fn spanish_catalog_is_applied() {
        let _g = crate::test_support::locale_test_guard();
        set_locale("C");
        let english = tr("document has no pages");
        set_locale("es_ES.UTF-8");
        let spanish = tr("document has no pages");
        assert_eq!(english, "document has no pages");
        assert_ne!(spanish, english);
        assert_eq!(
            spanish,
            Catalog::parse(include_str!("../../../po/es.po")).translate("document has no pages")
        );
        set_locale("en");
    }

    #[test]
    fn es_po_has_msgstr_for_every_msgid() {
        let cat = Catalog::parse(include_str!("../../../po/es.po"));
        assert!(cat.entries.len() > 20, "es.po looks empty");
        let mut translated = 0;
        for (id, s) in &cat.entries {
            assert!(!s.is_empty(), "empty msgstr for {id}");
            if s != id {
                translated += 1;
            } else {
                assert!(*id == "openpdf: {error}", "identity msgstr for {id}");
            }
        }
        assert!(
            translated >= cat.entries.len().saturating_sub(1),
            "es.po left too many msgids untranslated"
        );
    }

    #[test]
    fn available_locales_includes_es() {
        let langs = available_locales();
        assert!(langs.iter().any(|l| l == "en"), "{langs:?}");
        assert!(langs.iter().any(|l| l == "es"), "{langs:?}");
    }

    #[test]
    fn rust_sources_have_no_spanish_literals() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut hits = Vec::new();
        for rel in [
            "crates",
            "README.md",
            "po/openpdf.pot",
            "po/POTFILES.in",
            "po/LINGUAS",
        ] {
            walk_no_spanish(&root.join(rel), &mut hits);
        }
        assert!(
            hits.is_empty(),
            "Spanish belongs only in po/es.po:\n{}",
            hits.join("\n")
        );
    }

    fn is_spanish_letter(c: char) -> bool {
        matches!(
            c,
            '\u{00e1}'
                | '\u{00e9}'
                | '\u{00ed}'
                | '\u{00f3}'
                | '\u{00fa}'
                | '\u{00f1}'
                | '\u{00fc}'
                | '\u{00c1}'
                | '\u{00c9}'
                | '\u{00cd}'
                | '\u{00d3}'
                | '\u{00da}'
                | '\u{00d1}'
                | '\u{00dc}'
                | '\u{00bf}'
                | '\u{00a1}'
        )
    }

    fn walk_no_spanish(path: &Path, hits: &mut Vec<String>) {
        if path.is_dir() {
            let Ok(rd) = fs::read_dir(path) else { return };
            for ent in rd.flatten() {
                walk_no_spanish(&ent.path(), hits);
            }
            return;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ok = name.ends_with(".rs")
            || name.ends_with(".pot")
            || name.ends_with(".in")
            || name == "LINGUAS"
            || name.ends_with(".md")
            || name.ends_with(".toml");
        if !ok {
            return;
        }
        let Ok(src) = fs::read_to_string(path) else {
            return;
        };
        for (i, line) in src.lines().enumerate() {
            if line.chars().any(is_spanish_letter) {
                hits.push(format!("{}:{}: {line}", path.display(), i + 1));
            }
        }
    }

    #[test]
    fn detect_locale_reads_env() {
        // Not racing other tests on env: just parse helpers.
        assert_eq!(first_language("es_ES.UTF-8").as_deref(), Some("es_ES"));
        assert_eq!(first_language("C").as_deref(), None);
        assert_eq!(first_language("es:en").as_deref(), Some("es"));
        assert!(is_c_or_posix("C"));
        assert!(is_c_or_posix("C.UTF-8"));
        assert!(is_c_or_posix("POSIX"));
        assert!(!is_c_or_posix("es_ES.UTF-8"));
    }

    #[test]
    fn language_priority_es_en() {
        let tag = detect_locale_from(Some("es:en"), None, None, Some("en_US.UTF-8"));
        assert_eq!(tag, "es");
        crate::test_support::with_locale(&tag, || {
            let english = "document has no pages";
            assert_ne!(tr(english), english);
            assert_eq!(
                tr(english),
                Catalog::parse(include_str!("../../../po/es.po")).translate(english)
            );
        });
    }

    #[test]
    fn language_priority_fr_es() {
        let tag = detect_locale_from(Some("fr:es"), None, None, Some("en_US.UTF-8"));
        assert_eq!(tag, "es");
        crate::test_support::with_locale(&tag, || {
            let english = "document has no pages";
            assert_eq!(
                tr(english),
                Catalog::parse(include_str!("../../../po/es.po")).translate(english)
            );
        });
    }

    #[test]
    fn language_ignored_when_lang_is_c() {
        let tag = detect_locale_from(Some("es:en"), None, None, Some("C"));
        assert_eq!(tag, "en");
        let posix = detect_locale_from(Some("es:en"), None, None, Some("POSIX"));
        assert_eq!(posix, "en");
        let lc_all_c = detect_locale_from(Some("es"), Some("C"), None, Some("es_ES.UTF-8"));
        assert_eq!(lc_all_c, "en");
        crate::test_support::with_locale(&tag, || {
            assert_eq!(tr("document has no pages"), "document has no pages");
        });
    }

    #[test]
    fn language_wins_over_lc_when_locale_is_not_c() {
        assert_eq!(
            detect_locale_from(Some("es"), Some("de_DE.UTF-8"), None, None),
            "es"
        );
        assert_eq!(
            detect_locale_from(None, Some("es_ES.UTF-8"), None, None),
            "es_ES"
        );
    }

    #[test]
    fn tr_args_replaces_placeholders() {
        crate::test_support::with_locale("en", || {
            assert_eq!(tr_args("pages: {count}", &[("count", "2")]), "pages: 2");
        });
    }
}
