# Design gap: owner boards vs GTK4 + libadwaita

Rendered from the attached 2.8 MB HTML+JS board file (8 iframe mockups) with Chromium/Playwright. Boards clicked: viewer (View / Comment), settings (General / Viewing / Documents / Comments and signing), plus static Sign, Icon, Organize (dark), Home, About, and Download site. The mockups themselves are in Spanish; this report is English.

Verdict per element: **exact** / **approximate (why)** / **not possible in GTK4+libadwaita (why)**.

## Tokens

| Token | Design | GTK | Verdict |
| --- | --- | --- | --- |
| Accent (openPDF green) | `#0E7C66` / `rgb(14,124,102)` | `#0e7c66` | **exact** |
| Accent on dark | `#3CC4A2` | `.dark .suggested-action` | **exact** |
| Ink / title | `#1A1C20` | Adwaita `@window_fg_color` plus CSS | **approximate** — Adwaita still owns most body text |
| Highlight | `#FFE07A` | CSS color only | **exact** as a token; PDF marks are PDFium |
| Selected-certificate wash | `#DDF0EA` / `#0A5E4D` | `.cert-row.selected` | **exact** |
| Window / canvas (light) | `#EBEDF0` well, `#F8F9FA` chrome | page canvas `#EBEDF0` | **exact** |
| Window (dark) | `#1F2024` chrome, `#2A2C31` bars | dark canvas + chrome CSS | **approximate** — Adwaita dark palette plus our overrides |
| Cards | `#FFFFFF`, radius 8–12 px | Adwaita groups / CSS cards | **approximate** — same white cards, Adwaita row metrics |
| Pill / selected mode | filled `#0E7C66`, capsule | `.suggested-action` / `.pill` | **approximate** — GTK pill radius works; metrics differ slightly |
| Radii | 2 / 3 / 4 / 6 / 8 / 10 / 12 / 20 px | 8–12 px cards; CSD is the compositor | **approximate** |
| UI font | `Segoe UI Variable Text`, `Segoe UI`, `Instrument Sans`, `system-ui` | GTK/Adwaita interface font | **exact on Linux** — `system-ui` / `-apple-system` map to the platform UI font. **No bundled font.** |
| Document body font | `Source Serif 4`, Georgia | PDF built-in Times-Roman in the sample contract | **approximate** — page glyphs come from the PDF, not GTK CSS |
| UI sizes | 11–14 px / 500–600 | Adwaita ~1 rem | **approximate** |
| Outer board shadows (3) | `0 1px 3px rgba(0,0,0,.12)`; `0 1px 4px rgba(0,0,0,.12)`; `0 1px 4px rgba(0,0,0,.08)` | not in-app | **approximate** — those three are the HTML iframe drop shadows |
| In-app shadows | toolbar / card / page | cairo page shadow + CSS cards | **approximate** — GTK `box-shadow` is softer than CSS |
| Focus ring | `0 0 0 2px #fff, 0 0 0 4px #1A1C20` | Adwaita focus | **approximate** |

## App icon

Extracted from board **Icon and logo** (`viewBox="0 0 256 256"`): white page, folded green corner `#0E7C66`, open-`o` stroke (“a page with a folded corner and an open o”). Written to `data/icons/hicolor/scalable/apps/io.github.ej3mpl0.openpdf.svg` and `crates/openpdf-gtk/data/icons/io.github.ej3mpl0.openpdf.svg`.

| Element | Verdict |
| --- | --- |
| Page + fold + open-o paths | **exact** (same SVG) |
| Size set 64/48/32/24/16 | **approximate** — one scalable SVG |
| Dark-on-black wordmark | **approximate** — GNOME uses the same SVG |
| Taskbar / Windows store tile | **not possible** — Linux uses the hicolor / desktop Icon= id |

## Viewer

| Element | Verdict |
| --- | --- |
| Single chrome: document tabs + close + `+` + window controls | **approximate** — `AdwTabBar` + `GtkWindowControls` in a `WindowHandle`; no second `AdwHeaderBar` |
| Windows caption metrics (min/max/close) | **approximate** — GNOME CSD hit targets, not Windows |
| Header tool icons (open / save / print) | **approximate** — open works; save/print are disabled (no core ops) |
| Mode text tabs: View, Comment, Fill and sign, Organize, Edit, Protect, Tools | **approximate** — text pills; unimplemented modes disabled |
| Selected mode pill (green fill) | **exact** |
| Find field “Find in document Ctrl+F” + zoom “− 125 % +” + settings gear | **approximate** — on the tool row, not a Windows title bar |
| Left rail: thumbs / outline / attachments icons | **approximate** — thumbs and outline switch; attachments disabled (no API) |
| Thumbnail rail, page numbers, current page | **approximate** — cairo thumbs with real page tiles; selected = 2 px `#0E7C66` rounded stroke |
| Floating comment toolbar (Highlight, underline, colors) | **not possible** as product UI — `openpdf-core` has no annotations. Chrome is **hidden**, not a disabled fake. |
| Highlight `#FFE07A` on the page | **not possible** in GTK — marks are PDF content / future annots |
| Right comments panel + Reply + XFDF export | **not possible** — no comment model; panel **hidden** |
| Status “Page X of Y” left, view icons right | **approximate** — wired; dirty-dot not wired |
| Document page card, centered on `#EBEDF0` well, page shadow | **approximate** — cairo white page + offset shadow |

## Thumbnails

| Element | Verdict |
| --- | --- |
| Vertical strip of page previews | **exact** |
| 8 px rounded page, 2 px green selected border | **approximate** — cairo stroke |
| Page index under the card | **exact** |
| Real page content in thumbs | **exact** when PDFium is present |
| Lazy / visible-only decode | **exact** |

## Certificate picker

| Element | Verdict |
| --- | --- |
| Modal over dimmed viewer | **approximate** — `AdwWindow` modal + Adwaita dim |
| Privacy copy | **approximate** — local-token wording, not “Windows does not send…” |
| Radio list, selected row `#DDF0EA` | **approximate** — `AdwActionRow` + radio |
| Visible-signature checkbox + preview | **not possible** — `SignRequest` has no field-target appearance yet |
| Reason / Location fields | **pending core #37 / TSA** — Motivo / Lugar `AdwEntryRow`s wired to `SignRequest::with_reason` / `with_location` |
| PAdES B-B / B-T / B-LT dropdown | **pending core #37 / TSA** — B-B is the active level; B-T is shown but insensitive with a TSA tooltip; never silently falls back |
| Footer Cancel + Sign and save a copy | **approximate** — picker Cancel closes; Signing Cancel is wired to `CancelToken` (silent, document untouched) |
| “A signed copy is saved. The original is not modified.” | **exact** as a label |

## Verify

The boards have **no verify dialog**. GTK shows a toast from `Verifier::verify` on the original bytes. A design-matched panel would be invented; we keep the toast.

| Element | Verdict |
| --- | --- |
| Verify toast / result string | **approximate** — Adwaita toast |
| Dedicated verify board | **not possible** — not in the design |

## Settings / About

| Element | Verdict |
| --- | --- |
| In-tab Settings page with left nav | **not possible** as Windows Settings — GNOME uses `AdwPreferencesWindow` |
| Search box | **exact** |
| Language combo (es / en / system) | **exact** |
| Theme segmented System / Light / Dark | **approximate** — combo today |
| On-open / default app / new window vs tab | **approximate** — restore-last exists; default-app is the desktop file |
| Viewing: zoom, layout, night invert, last page, AA | **approximate** — zoom + layout exist |
| About as a settings section | **approximate** — `AdwAboutWindow` |

## Home + tools

| Element | Verdict |
| --- | --- |
| Home tab | **approximate** — only added when no document is open |
| Dashed drop zone | **approximate** — CSS dashed border + `GtkDropTarget` |
| Open (green) + Create PDF from images | **approximate** — Open works; images wait on core |
| Recent list | **approximate** |
| Tool card grid | **approximate** — most tiles toast “Waiting on openpdf-core” |
| Combine / split / compress / watermark / redact | **not possible** as working tools |

## Organize pages (dark)

| Element | Verdict |
| --- | --- |
| Dark theme chrome | **approximate** |
| 6-across page grid, multi-select, rotate / extract / delete | **not possible** as that surface |
| “Insert pages” dashed tile | **not possible** — no insert API |

## Marketing site

| Element | Verdict |
| --- | --- |
| Whole page | **not possible** — not an application surface |

## Screenshots

Four states (viewer, thumbnails, certificate picker, verify) at 1280x800 with `corpus/contrato.pdf` at 125 percent. Spanish (`LANGUAGE=es`) files sit next to English (`LANGUAGE=en`) `*-en.png` variants under `docs/screenshots/design-compare/`. Design boards stay in the original Spanish mockup language.

## Startup / RAM

Deferred chrome icons, deferred design CSS (skipped in `--measure`), and lazy thumbs stay. Target: cairo startup median ≤ 300 ms, peak RSS < 150 MiB with the SVG loader on. Gilfoyle’s CI RSS gate is 160 MiB (do not edit `ci.yml`). The in-tree `--measure` fail gate remains 250 MiB so hosted runners do not flake.
