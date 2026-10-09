# openPDF

App nativa de PDF para Linux. Rust + GTK4/libadwaita, render con pdfium-render.

- `openpdf-core`: documento, teselas, texto, undo, merge/split, `Signer`
- `openpdf-cli`: `openpdf info|render|text|merge|split`
- `openpdf-gtk`: interfaz
- `openpdf-sign`: firma PKCS#11 (DNIe, FNMT)

Regla: gtk y sign dependen de core, nunca entre ellos.

PDFium se carga en tiempo de ejecución (`OPENPDF_PDFIUM`, junto al binario, o el sistema). No se usa MuPDF.
