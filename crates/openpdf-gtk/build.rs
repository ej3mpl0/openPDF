// SPDX-License-Identifier: MPL-2.0
fn main() {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let dest = format!("{out}/icons.gresource");
    let xml = "data/icons.gresource.xml";
    println!("cargo:rerun-if-changed={xml}");
    println!("cargo:rerun-if-changed=data/icons/scalable/actions/document-open-symbolic.svg");
    println!("cargo:rerun-if-changed=data/icons/scalable/actions/sidebar-show-symbolic.svg");
    println!("cargo:rerun-if-changed=data/icons/scalable/actions/open-menu-symbolic.svg");
    let status = std::process::Command::new("glib-compile-resources")
        .args(["--sourcedir", "data", "--target", &dest, xml])
        .status()
        .expect("glib-compile-resources (libglib2.0-dev) is required to bundle icons");
    assert!(
        status.success(),
        "glib-compile-resources failed with {status}"
    );
}
