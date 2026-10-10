// SPDX-License-Identifier: MPL-2.0
fn main() {
    compile(
        "data/icons-chrome.gresource.xml",
        "icons-chrome.gresource",
        &[
            "data/icons/scalable/actions/document-open-symbolic.svg",
            "data/icons/scalable/actions/sidebar-show-symbolic.svg",
        ],
    );
    compile(
        "data/icons-deferred.gresource.xml",
        "icons-deferred.gresource",
        &[
            "data/icons/scalable/actions/open-menu-symbolic.svg",
            "data/icons/io.github.ej3mpl0.openpdf.svg",
        ],
    );
}

fn compile(xml: &str, dest_name: &str, inputs: &[&str]) {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let dest = format!("{out}/{dest_name}");
    println!("cargo:rerun-if-changed={xml}");
    for input in inputs {
        println!("cargo:rerun-if-changed={input}");
    }
    let status = std::process::Command::new("glib-compile-resources")
        .args(["--sourcedir", "data", "--target", &dest, xml])
        .status()
        .expect("glib-compile-resources (libglib2.0-dev) is required to bundle icons");
    assert!(
        status.success(),
        "glib-compile-resources failed with {status}"
    );
}
