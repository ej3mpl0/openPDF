# Design gap: owner boards vs GTK4 + libadwaita

Rendered from the attached 2.8 MB HTML+JS board file (8 iframe mockups) with Chromium/Playwright. Boards clicked: viewer **Ver** / **Comentar**, settings **General** / **Visualización** / **Documentos** / **Comentarios y firma**, plus static **Firmar**, **Icono**, **Organizar (oscuro)**, **Inicio**, **Acerca de**, **Web**.

Verdict per element: **exact** / **approximate (why)** / **not possible in GTK4+libadwaita (why)**.

## Tokens

| Token | Design | GTK today | Verdict |
| --- | --- | --- | --- |
| Accent (verde openPDF) | `#0E7C66` / `rgb(14,124,102)` | `#0e7c66` already | **exact** |
| Accent on dark | `#3CC4A2` | unused | **exact** once CSS maps `@accent_color` in `.dark` |
| Ink / title | `#1A1C20` | Adwaita `@window_fg_color` | **approximate** — Adwaita owns body text; we set window fg to this |
| Highlight | `#FFE07A` | none | **exact** as a CSS color; PDF highlight paint is PDFium, not CSS |
| Selected-cert wash | `#DDF0EA` / `#0A5E4D` | none | **exact** via CSS class |
| Window / canvas (light) | `#EBEDE0` chrome, `#F8F9FA` / `#F4F5F7` page well | page canvas `#c8c5bf` | **exact** after CSS |
| Window (dark) | `#1F2024` chrome, `#2A2C31` bars | `#1c1c1c` | **approximate** — Adwaita dark palette plus our canvas override |
| Cards | `#FFFFFF`, radius 8–12 px | Adwaita `PreferencesGroup` | **approximate** — same white cards, Adwaita row metrics |
| Pill / selected mode | filled `#0E7C66`, 999 px capsule | `.suggested-action` / `.pill` | **approximate** — GTK pills are not 999 px CSS; `border-radius: 999px` works on GtkButton |
| Radii | 2 / 3 / 4 / 6 / 8 / 10 / 12 / 20 px | Adwaita 6–12 | **approximate** — we can set 8 px cards and 6 px inputs; CSD window radius is the compositor |
| UI font | `Segoe UI Variable Text`, `Segoe UI`, `Instrument Sans`, `system-ui` | GTK/Adwaita interface font | **exact on Linux** — `system-ui` / `-apple-system` map to the platform UI font. **No bundled font.** Instrument Sans / Segoe are Windows/mock fonts; bundling them would need OFL + sha256 and the boards do not require a custom face. |
| Document body font | `Source Serif 4`, Georgia | PDFium embedded fonts | **not possible** — page glyphs come from the PDF, not GTK CSS |
| UI sizes | 11–14 px / 500–600 | Adwaita 1 rem | **approximate** — we can set 13 px chrome; default Adwaita is slightly larger |
| Outer board shadows (3) | `0 1px 3px rgba(0,0,0,.12)`; `0 1px 4px rgba(0,0,0,.12)`; `0 1px 4px rgba(0,0,0,.08)` | none | **approximate** — those three are the **board iframe** drop shadows in the HTML wrapper, not in-app chrome. GTK can do equivalent `box-shadow` on cards/dialogs. |
| In-app shadows | `0 2px 8px rgba(20,24,31,.08)` (comment toolbar); `0 1px 2px /.08 + 0 6px 20px /.06` (cards); `0 1px 3px /.20` (thumbs); dialog `0 12px 40px rgba(15,17,20,.24)` | Adwaita elevation | **approximate** — GTK4 `box-shadow` works; stacked multi-shadows are softer than CSS, and Adwaita dialogs already dim the parent |
| Focus ring | `0 0 0 2px #fff, 0 0 0 4px #1A1C20` | Adwaita focus | **approximate** — libadwaita owns focus; we can tint, not replace the two-ring Windows style |

## App icon

Extracted from board **Icono y logotipo** (`viewBox="0 0 256 256"`): white page, folded green corner `#0E7C66`, open-`o` stroke (the “página con la esquina doblada y una o abierta”). Written to `data/icons/hicolor/scalable/apps/io.github.ej3mpl0.openpdf.svg` and `crates/openpdf-gtk/data/icons/io.github.ej3mpl0.openpdf.svg`.

| Element | Verdict |
| --- | --- |
| Page + fold + open-o paths | **exact** (same SVG) |
| Size set 64/48/32/24/16 | **approximate** — one scalable SVG; hicolor does not ship every raster |
| Dark-on-black wordmark | **approximate** — GNOME uses the same SVG; no separate dark asset unless we add `-symbolic` |
| Taskbar / Windows store tile | **not possible** — Linux uses the hicolor / desktop Icon= id, not a Windows taskbar |

## Viewer (board: Visor · modo Comentar / Ver)

| Element | Verdict |
| --- | --- |
| Multi-document tabs | **approximate** — `AdwTabView` + `AdwTabBar` exist; they are GNOME tabs, not Chrome/Windows tabs (no favicon-on-tab, no `+` on the tab strip in the same metrics) |
| Windows caption (min/max/close on the right, icon on the left) | **not possible** — CSD is `AdwHeaderBar` + the compositor; button side and hit targets are GNOME |
| Header tool icons (open / print / …) | **approximate** — we already defer non-chrome icons; print is not a GTK action yet |
| Mode strip: Ver / Comentar / Rellenar y firmar / Organizar / Editar / Proteger / Herramientas | **approximate** — text pills can be GTK buttons; Comentar / Organizar / Editar / Proteger have no core API, so they stay disabled or toast |
| Selected mode pill (green fill) | **exact** via CSS |
| Find field + Ctrl+F + zoom − 125 % + | **approximate** — `GtkSearchBar` + zoom buttons exist; they live on the document chrome, not the Windows title bar |
| Thumbnail rail, page numbers, current page | **approximate** — we paint thumbs in cairo; selected treatment can be a 2 px `#0E7C66` rounded stroke (design) instead of today’s filled green slab |
| Bookmark / annotation rail icons | **approximate** — outline list exists; Lucide-style 15 px icons become Adwaita symbolic |
| Floating comment toolbar (Resaltar, underline, colors) | **not possible** as product UI until core annotations land; overlay widget itself is possible (`GtkOverlay`) |
| Highlight `#FFE07A` on the page | **not possible** in GTK — marks are PDF content / future annots, not CSS |
| Right **Comentarios** panel + Responder + XFDF export | **not possible** — no comment model in `openpdf-core` |
| Status “Página 3 de 12 · Cambios sin guardar” | **approximate** — we have a page label; dirty-dot is not wired |
| Document page card, 8 px radius, centered on grey well | **approximate** — cairo page rect + CSS well color; rounded clip on a `DrawingArea` is extra cairo, not CSS |

## Thumbnails

| Element | Verdict |
| --- | --- |
| Vertical strip of page previews | **exact** (already) |
| 8 px rounded page, 2 px green selected border | **approximate** — cairo stroke, not CSS `border-radius` |
| Page index under the card | **exact** |
| Lazy / visible-only decode | **exact** (deferred loader stays on) |

## Certificate picker (board: Firmar con certificado)

| Element | Verdict |
| --- | --- |
| Modal over dimmed viewer | **approximate** — `AdwWindow` modal + Adwaita dim; not `rgba(15,17,20,.42)` custom scrim |
| Title + Windows privacy copy | **approximate** — we already explain PIN; “Windows no envía…” is the wrong OS |
| Radio list, selected row `#DDF0EA` | **approximate** — `AdwActionRow` + radio prefix; Adwaita rows, not custom 12 px cards |
| Visible-signature checkbox + preview | **not possible** — `SignRequest` has no visible-appearance / field target |
| Motivo / Lugar fields | **not possible** — not on the core request |
| PAdES B-B / B-T / B-LT dropdown | **not possible** — level is the signer’s, not a GTK enum |
| Footer Cancel + **Firmar y guardar copia** green pill | **approximate** — we can add that footer; today a row click signs immediately |
| “Se guarda una copia. El original no se modifica.” | **exact** as a label (we already persist a sibling) |

## Verify

The boards have **no verify dialog**. GTK shows a toast from `Verifier::verify` on the original bytes. A design-matched panel would be invented; we keep the toast and tint it with the accent.

| Element | Verdict |
| --- | --- |
| Verify toast / result string | **approximate** — Adwaita toast, not a Windows flyout |
| Dedicated verify board | **not possible** — not in the design; do not invent chrome |

## Settings (boards: Ajustes · idioma/tema, Acerca de)

| Element | Verdict |
| --- | --- |
| In-tab Settings page with left nav | **not possible** as Windows Settings — GNOME uses `AdwPreferencesWindow` (sidebar pages). Recreating a document-tab “Ajustes” would fight `AdwTabView` and a11y |
| Search box | **exact** — PreferencesWindow search |
| Language combo (es / en / system) | **exact** (already) |
| Theme segmented Sistema / Claro / Oscuro | **approximate** — we have a combo; `AdwToggleGroup` (libadwaita 1.7+) or three toggles can get closer |
| “Al abrir” / default-app / new window vs tab | **approximate** — restore-last exists; Windows default-app and “new window” are portal / desktop-file, not in-window |
| Viewing: zoom, layout, night invert, last page, AA | **approximate** — zoom + layout exist; night invert / AA are PDFium flags, not CSS |
| Documents / Comments / OCR / Advanced extra pages | **approximate** — we have Advanced (tile/cache); OCR/comments wait on core |
| About as a settings section (icon, heart, licenses) | **approximate** — `AdwAboutWindow` is the GNOME about; we can set the new icon, version, website, credits |

## Home + tools (board: Inicio y herramientas)

| Element | Verdict |
| --- | --- |
| Home tab | **approximate** — pinned `AdwTabView` page |
| Dashed drop zone “Arrastra un PDF” | **approximate** — GTK has no native dashed drop target; CSS dashed border + `GtkDropTarget` is close |
| **Abrir archivo** green + **Crear PDF desde imágenes** | **approximate** — Open is exact; image→PDF waits on core |
| Recientes list with relative dates | **approximate** — we list names; glib relative dates are doable |
| Tool card grid (12 tiles) | **approximate** — `GtkFlowBox` + cards; most tiles toast “Waiting on openpdf-core” |
| Combine / split / compress / watermark / redact | **not possible** as working tools — no core ops |

## Organize pages · dark (board)

| Element | Verdict |
| --- | --- |
| Dark theme chrome | **approximate** — `AdwStyleManager` dark + our canvas tokens |
| 6-across page grid, multi-select, rotate / extract / delete | **not possible** as that surface — we rotate/delete the current page only; no organize grid |
| “Insertar páginas” dashed tile | **not possible** — no insert API |

## Marketing site (board: Web de descarga)

| Element | Verdict |
| --- | --- |
| Whole page | **not possible** — not an application surface. Icon/wordmark reuse only. |

## Startup / RAM (constraints, not design)

Deferred chrome icons and lazy thumbs stay. Target: cairo startup median ≤ 300 ms, peak RSS < 150 MiB with the SVG loader on. Matching the design is CSS + a thin widget restyle, not new icon packs at startup.
