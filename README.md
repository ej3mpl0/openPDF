# openPDF

App nativa de PDF para Linux. Rust + GTK4/libadwaita, render con pdfium-render.

- `openpdf-core`: documento, render, texto, CLI
- `openpdf-gtk`: interfaz
- `openpdf-sign`: firma PKCS#11 (DNIe, FNMT)

Regla: gtk y sign dependen de core, nunca entre ellos.
