use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    env!("CARGO_BIN_EXE_openpdf").into()
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(name)
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("run openpdf")
}

#[test]
fn info_two_page() {
    let out = run(&["info", corpus("two_page.pdf").to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pages: 2"), "{stdout}");
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
