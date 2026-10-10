// SPDX-License-Identifier: MPL-2.0
fn main() {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let dest = format!("{out}/icons.gresource");
    let xml = "data/icons.gresource.xml";
    println!("cargo:rerun-if-changed={xml}");
    for name in [
        "document-open-symbolic",
        "sidebar-show-symbolic",
        "open-menu-symbolic",
        "go-up-symbolic",
        "go-down-symbolic",
        "zoom-in-symbolic",
        "zoom-out-symbolic",
        "edit-delete-symbolic",
        "object-rotate-right-symbolic",
        "dialog-password-symbolic",
    ] {
        println!("cargo:rerun-if-changed=data/icons/scalable/actions/{name}.svg");
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
