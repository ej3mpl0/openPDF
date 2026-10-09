use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let po_dir = manifest.join("../../po");
    println!("cargo:rerun-if-changed={}", po_dir.display());

    let mut entries = Vec::new();
    if let Ok(rd) = fs::read_dir(&po_dir) {
        let mut files: Vec<_> = rd.flatten().collect();
        files.sort_by_key(|e| e.file_name());
        for ent in files {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if let Some(code) = name.strip_suffix(".po") {
                if code == "openpdf" {
                    continue;
                }
                let path = ent.path();
                println!("cargo:rerun-if-changed={}", path.display());
                entries.push((
                    code.to_string(),
                    path.canonicalize().unwrap_or(path).display().to_string(),
                ));
            }
        }
    }

    let mut out = String::from("pub static EMBEDDED_PO: &[(&str, &str)] = &[\n");
    for (code, path) in &entries {
        out.push_str(&format!(
            "    (\"{}\", include_str!(\"{}\")),\n",
            code,
            path.replace('\\', "/")
        ));
    }
    out.push_str("];\n");

    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("embedded_po.rs");
    fs::write(dest, out).unwrap();
}
