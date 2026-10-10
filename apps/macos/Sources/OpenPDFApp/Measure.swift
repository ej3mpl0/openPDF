import AppKit
import Darwin
import Foundation

enum Measure {
    static let defaultPages = 32
    static let maxStartupMs: UInt64 = 8_000
    static let maxRssBytes: UInt64 = 250 * 1024 * 1024
    static let minVisibleTiles = 1
    static let minThumbTiles = 3

    struct Report: CustomStringConvertible {
        var pages: Int
        var startupMs: UInt64
        var rssBytes: UInt64
        var visibleReady: Int
        var visibleTotal: Int
        var thumbReady: Int
        var thumbTotal: Int
        /// AppKit activate / measure entry, milliseconds from process start.
        var uiActivateMs: UInt64 = 0
        /// `orderFront` after chrome is built.
        var uiWindowPresentMs: UInt64 = 0
        /// First `displayIfNeeded` of the empty canvas.
        var uiFirstFrameMs: UInt64 = 0
        /// First visible tile in cache.
        var uiFirstTileMs: UInt64 = 0
        /// NSApplication bootstrap (AppKit analogue of adw::init).
        var uiAdwInitMs: UInt64 = 0
        /// Unused on AppKit; kept so the report matches openpdf-gtk.
        var uiCssMs: UInt64 = 0
        /// NSWindow + canvas construction.
        var uiWindowBuildMs: UInt64 = 0
        var uiDocLoadStartMs: UInt64 = 0
        var uiDocLoadedMs: UInt64 = 0
        var uiFirstTileRequestedMs: UInt64 = 0
        var uiFirstTileRenderedMs: UInt64 = 0
        var uiFirstTextureUploadedMs: UInt64 = 0
        var ok: Bool
        var error: String?

        var description: String {
            var lines = [
                "mode=ui",
                "pages=\(pages)",
                "startup_ms=\(startupMs)",
                "peak_rss_bytes=\(rssBytes)",
                String(format: "peak_rss_mb=%.1f", Double(rssBytes) / (1024.0 * 1024.0)),
                "visible_tiles=\(visibleReady)/\(visibleTotal)",
                "thumb_tiles=\(thumbReady)/\(thumbTotal)",
                "ui_activate_ms=\(uiActivateMs)",
                "ui_window_present_ms=\(uiWindowPresentMs)",
                "ui_first_frame_ms=\(uiFirstFrameMs)",
                "ui_first_tile_ms=\(uiFirstTileMs)",
                "ui_adw_init_ms=\(uiAdwInitMs)",
                "ui_css_ms=\(uiCssMs)",
                "ui_window_build_ms=\(uiWindowBuildMs)",
                "ui_doc_load_start_ms=\(uiDocLoadStartMs)",
                "ui_doc_loaded_ms=\(uiDocLoadedMs)",
                "ui_first_tile_requested_ms=\(uiFirstTileRequestedMs)",
                "ui_first_tile_rendered_ms=\(uiFirstTileRenderedMs)",
                "ui_first_texture_uploaded_ms=\(uiFirstTextureUploadedMs)",
                "limit_startup_ms=\(maxStartupMs)",
                "limit_peak_rss_bytes=\(maxRssBytes)",
                "ok=\(ok)",
            ]
            if let error {
                lines.append("error=\(error)")
            }
            return lines.joined(separator: "\n") + "\n"
        }

        mutating func evaluate() {
            var err = error
            if visibleReady < minVisibleTiles {
                err = err ?? "visible tiles \(visibleReady)/\(visibleTotal) (need ≥ \(minVisibleTiles))"
            }
            if thumbReady < minThumbTiles {
                err = err ?? "thumbnail tiles \(thumbReady)/\(thumbTotal) (need ≥ \(minThumbTiles))"
            }
            if startupMs > maxStartupMs {
                err = err ?? "startup \(startupMs) ms exceeds \(maxStartupMs) ms"
            }
            if rssBytes > maxRssBytes {
                err = err ?? "peak RSS \(rssBytes) exceeds \(maxRssBytes)"
            }
            error = err
            ok = err == nil
        }
    }

    private static var processStart = DispatchTime.now()
    private static var processStartPinned = false

    static func markProcessStart() {
        if !processStartPinned {
            processStart = DispatchTime.now()
            processStartPinned = true
        }
    }

    static func processElapsedMs() -> UInt64 {
        (DispatchTime.now().uptimeNanoseconds - processStart.uptimeNanoseconds) / 1_000_000
    }

    static func bootstrapApp() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        _ = app
    }

    static func run(pages: Int) -> Int32 {
        markProcessStart()
        let uiAdwInitMs = processElapsedMs()
        var report = Report(
            pages: pages,
            startupMs: 0,
            rssBytes: 0,
            visibleReady: 0,
            visibleTotal: 0,
            thumbReady: 0,
            thumbTotal: 0,
            uiAdwInitMs: uiAdwInitMs,
            uiCssMs: uiAdwInitMs,
            ok: false,
            error: nil
        )
        do {
            // Chrome first: do not block present() on PDFium or tile I/O.
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
                styleMask: [.titled, .closable, .resizable],
                backing: .buffered,
                defer: false
            )
            window.isReleasedWhenClosed = false
            window.title = "openPDF measure"
            let canvas = TileCanvasView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
            window.contentView = canvas
            report.uiWindowBuildMs = processElapsedMs()
            report.uiActivateMs = report.uiWindowBuildMs
            window.orderFront(nil)
            report.uiWindowPresentMs = processElapsedMs()
            canvas.layoutSubtreeIfNeeded()
            canvas.displayIfNeeded()
            report.uiFirstFrameMs = processElapsedMs()

            report.uiDocLoadStartMs = processElapsedMs()
            let pdf = try writeMeasurePdf(pages: max(pages, 2))
            defer { try? FileManager.default.removeItem(atPath: pdf) }

            let lib = OpenPdf()
            let doc = try lib.open(path: pdf)
            let pageInfos = try doc.pages()
            report.uiDocLoadedMs = processElapsedMs()
            canvas.setDocument(doc, pages: pageInfos, zoom: 1.0)
            canvas.layoutSubtreeIfNeeded()

            let viewport = CGRect(x: 0, y: 0, width: 800, height: 600)
            var visKeys: [TileCoord] = []
            if let first = pageInfos.first {
                visKeys = canvas.keysForPage(first, zoom: 1.0, viewport: viewport)
            }
            if visKeys.isEmpty, let first = pageInfos.first {
                visKeys = [TileCoord(page: first.index, zoom: 1.0, tx: 0, ty: 0)]
            }
            report.uiFirstTileRequestedMs = processElapsedMs()
            let vis = canvas.renderKeysNow(visKeys)
            report.uiFirstTileRenderedMs = processElapsedMs()
            report.uiFirstTextureUploadedMs = report.uiFirstTileRenderedMs
            report.uiFirstTileMs = processElapsedMs()
            report.visibleReady = vis.ready
            report.visibleTotal = vis.total

            let thumbZoom: Float = 0.18
            let thumbN = min(4, pageInfos.count)
            var thumbReady = 0
            for i in 0 ..< thumbN {
                if (try? doc.renderTile(page: pageInfos[i].index, zoom: thumbZoom, tx: 0, ty: 0)) != nil {
                    thumbReady += 1
                }
            }
            report.thumbReady = thumbReady
            report.thumbTotal = thumbN

            report.startupMs = processElapsedMs()
            report.rssBytes = peakRssBytes()
            report.evaluate()
        } catch {
            report.startupMs = processElapsedMs()
            report.rssBytes = peakRssBytes()
            report.error = error.localizedDescription
            report.evaluate()
        }
        FileHandle.standardOutput.write(Data(report.description.utf8))
        return report.ok ? 0 : 1
    }

    static func writeMeasurePdf(pages: Int) throws -> String {
        let hello = try corpusPdf("hello.pdf")
        let lib = OpenPdf()
        let src = try lib.open(path: hello)
        var doc = src
        let range = [PageRange(start: 0, end: 0)]
        while try Int(doc.pageCount()) < pages {
            doc = try doc.insertPages(at: try doc.pageCount(), source: src, ranges: range)
        }
        let dest = FileManager.default.temporaryDirectory
            .appendingPathComponent("openpdf-measure-\(ProcessInfo.processInfo.processIdentifier)-\(pages).pdf")
            .path
        try doc.save(path: dest)
        return dest
    }

    static func corpusPdf(_ name: String) throws -> String {
        let env = ProcessInfo.processInfo.environment["OPENPDF_CORPUS"]
        var candidates: [URL] = []
        if let env {
            candidates.append(URL(fileURLWithPath: env).appendingPathComponent(name))
        }
        let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        candidates.append(cwd.appendingPathComponent("corpus").appendingPathComponent(name))
        candidates.append(cwd.appendingPathComponent("../../corpus").appendingPathComponent(name))
        if let exe = Bundle.main.executableURL {
            candidates.append(exe.deletingLastPathComponent().appendingPathComponent("corpus").appendingPathComponent(name))
            candidates.append(
                exe.deletingLastPathComponent()
                    .appendingPathComponent("../../../corpus")
                    .appendingPathComponent(name)
            )
        }
        if let found = candidates.first(where: { FileManager.default.fileExists(atPath: $0.path) }) {
            return found.path
        }
        throw NSError(
            domain: "openpdf",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: "corpus/\(name) is missing; cannot build the measure document"]
        )
    }

    /// macOS `ru_maxrss` is bytes (Linux is KiB).
    static func peakRssBytes() -> UInt64 {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        return UInt64(usage.ru_maxrss)
    }
}
