use std::path::{Path, PathBuf};
use std::process::Command;

use openpdf_core::i18n::Catalog;

fn bin() -> PathBuf {
    env!("CARGO_BIN_EXE_openpdf").into()
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(name)
}

fn es_catalog() -> Catalog {
    Catalog::parse(include_str!("../../../po/es.po"))
}

fn run(args: &[&str]) -> std::process::Output {
    run_locale("C", args)
}

fn run_locale(locale: &str, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env("LC_ALL", locale)
        .env("LC_MESSAGES", locale)
        .env("LANG", locale)
        .env_remove("LANGUAGE")
        .output()
        .expect("run openpdf")
}

fn run_language(language: &str, lang: &str, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .env_remove("LC_ALL")
        .env_remove("LC_MESSAGES")
        .env("LANG", lang)
        .env("LANGUAGE", language)
        .output()
        .expect("run openpdf")
}

fn pages_msgid() -> &'static str {
    "pages: {count}"
}

fn spanish_pages_line() -> String {
    es_catalog()
        .translate(pages_msgid())
        .replace("{count}", "2")
}

#[test]
fn info_two_page() {
    let out = run(&["info", corpus("two_page.pdf").to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pages: 2"), "{stdout}");
}

#[test]
fn info_follows_spanish_catalog_without_hardcoded_msgstr() {
    let file = corpus("two_page.pdf");
    let en = run_locale("C", &["info", file.to_str().unwrap()]);
    let es = run_locale("es_ES.UTF-8", &["info", file.to_str().unwrap()]);
    assert!(en.status.success(), "{}", String::from_utf8_lossy(&en.stderr));
    assert!(es.status.success(), "{}", String::from_utf8_lossy(&es.stderr));

    let en_out = String::from_utf8_lossy(&en.stdout);
    let es_out = String::from_utf8_lossy(&es.stdout);
    assert_ne!(en_out, es_out);

    let cat = es_catalog();
    let pages = cat.translate("pages: {count}").replace("{count}", "2");
    assert_ne!(pages, "pages: 2");
    assert!(es_out.contains(&pages), "{es_out}");
    assert!(!es_out.contains("pages: 2"), "{es_out}");
    assert!(en_out.contains("pages: 2"), "{en_out}");
}

#[test]
fn help_follows_locale() {
    let en = run_locale("C", &["--help"]);
    let es = run_locale("es", &["--help"]);
    assert!(en.status.success(), "{}", String::from_utf8_lossy(&en.stderr));
    assert!(es.status.success(), "{}", String::from_utf8_lossy(&es.stderr));

    let en_out = String::from_utf8_lossy(&en.stdout);
    let es_out = String::from_utf8_lossy(&es.stdout);
    assert_ne!(en_out.as_ref(), es_out.as_ref());

    let about = "openPDF command-line tools (share openpdf-core with the GTK app)";
    assert!(en_out.contains(about), "{en_out}");
    let cat = es_catalog();
    let translated = cat.translate(about);
    assert_ne!(translated, about);
    assert!(es_out.contains(&translated), "{es_out}");
    assert!(!es_out.contains(about), "{es_out}");
}

#[test]
fn info_language_es_en() {
    let file = corpus("two_page.pdf");
    let out = run_language("es:en", "en_US.UTF-8", &["info", file.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pages = spanish_pages_line();
    assert_ne!(pages, "pages: 2");
    assert!(stdout.contains(&pages), "{stdout}");
    assert!(!stdout.contains("pages: 2"), "{stdout}");
}

#[test]
fn info_language_fr_es() {
    let file = corpus("two_page.pdf");
    let out = run_language("fr:es", "en_US.UTF-8", &["info", file.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pages = spanish_pages_line();
    assert!(stdout.contains(&pages), "{stdout}");
    assert!(!stdout.contains("pages: 2"), "{stdout}");
}

#[test]
fn info_language_ignored_when_lang_is_c() {
    let file = corpus("two_page.pdf");
    let out = run_language("es:en", "C", &["info", file.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pages: 2"), "{stdout}");
    let pages = spanish_pages_line();
    assert!(!stdout.contains(&pages), "{stdout}");
}

#[test]
fn merge_and_split() {
    let dir = tempfile();
    let merged = dir.join("merged.pdf");
    let out = run(&[
        "merge",
        corpus("hello.pdf").to_str().unwrap(),
        corpus("two_page.pdf").to_str().unwrap(),
        "-o",
        merged.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let parts = dir.join("parts");
    let out = run(&[
        "split",
        merged.to_str().unwrap(),
        "-o",
        parts.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_dir(&parts).unwrap().count(), 3);
}

#[test]
fn render_and_text_if_pdfium() {
    if EngineProbe::missing() {
        return;
    }
    let png = tempfile().join("page.png");
    let out = run(&[
        "render",
        corpus("hello.pdf").to_str().unwrap(),
        "--page",
        "1",
        "-o",
        png.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(png.is_file());
    let header = std::fs::read(&png).unwrap();
    assert_eq!(&header[..8], b"\x89PNG\r\n\x1a\n");

    let out = run(&["text", corpus("hello.pdf").to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Hello") || stdout.contains("openPDF"), "{stdout}");
}

struct EngineProbe;

impl EngineProbe {
    fn missing() -> bool {
        match openpdf_core::Engine::new() {
            Ok(_) => false,
            Err(e) => {
                if std::env::var("CI").is_ok() {
                    panic!("PDFium required in CI: {e}");
                }
                eprintln!("skipping CLI PDFium test: {e}");
                true
            }
        }
    }
}

fn tempfile() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "openpdf-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
