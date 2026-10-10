use crate::i18n::t;
use crate::Lang;
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

pub fn present(parent: &impl IsA<gtk::Window>, lang: Lang) {
    let win = adw::AboutWindow::new();
    win.set_transient_for(Some(parent));
    win.set_modal(true);
    win.set_application_name("openPDF");
    win.set_application_icon(crate::APP_ID);
    win.set_version(env!("CARGO_PKG_VERSION"));
    win.set_developer_name("Jairo");
    win.set_comments(t(
        lang,
        "Local PDF app. No accounts, no cloud, no telemetry.",
    ));
    win.set_license_type(gtk::License::Mpl20);
    win.set_website("https://github.com/ej3mpl0/openPDF");
    win.set_issue_url("https://github.com/ej3mpl0/openPDF/issues");
    win.set_developers(&["Jairo"]);
    win.add_acknowledgement_section(
        Some("Third-party"),
        &[
            "GTK 4 — LGPL-2.1+",
            "libadwaita — LGPL-2.1+",
            "PDFium via pdfium-render — BSD/Apache-2.0 (through openpdf-core)",
        ],
    );
    win.set_debug_info(&format!(
        "crate: openpdf-gtk {}\nui: GTK4 + libadwaita\nrenderer: openpdf-core Engine (PDFium tiles)\ncore API: {}\n",
        env!("CARGO_PKG_VERSION"),
        crate::core_needed::CORE_API_VERSION_EXPECTED
    ));
    win.present();
}
