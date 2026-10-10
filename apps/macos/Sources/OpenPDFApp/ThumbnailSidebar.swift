import AppKit
import SwiftUI

/// Sidebar thumbs: one low-zoom tile per page, still via core LRU `renderTile`.
struct ThumbnailSidebar: View {
    var document: PdfDocument?
    var pages: [PageInfo]
    var currentPage: Int
    var onSelect: (Int) -> Void
    var onReady: (Int, Int) -> Void

    private let thumbZoom: Float = 0.18

    var body: some View {
        ScrollView {
            LazyVStack(spacing: 8) {
                ForEach(Array(pages.enumerated()), id: \.element.index) { idx, page in
                    Button {
                        onSelect(idx)
                    } label: {
                        VStack(spacing: 4) {
                            ThumbnailTile(
                                document: document,
                                page: page,
                                zoom: thumbZoom,
                                onRendered: { reportReady() }
                            )
                            .frame(width: 96, height: thumbHeight(page))
                            Text("\(idx + 1)")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .padding(4)
                        .background(
                            RoundedRectangle(cornerRadius: 6)
                                .fill(idx == currentPage ? Color.accentColor.opacity(0.15) : Color.clear)
                        )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(8)
        }
        .onAppear { reportReady() }
    }

    private func thumbHeight(_ page: PageInfo) -> CGFloat {
        let w = CGFloat(page.widthPt * thumbZoom)
        let h = CGFloat(page.heightPt * thumbZoom)
        guard w > 0 else { return 128 }
        return 96 * h / w
    }

    private func reportReady() {
        guard let document else {
            onReady(0, 0)
            return
        }
        let n = min(4, pages.count)
        var ready = 0
        for i in 0 ..< n {
            if (try? document.renderTile(page: pages[i].index, zoom: thumbZoom, tx: 0, ty: 0)) != nil {
                ready += 1
            }
        }
        onReady(ready, n)
    }
}

private struct ThumbnailTile: NSViewRepresentable {
    var document: PdfDocument?
    var page: PageInfo
    var zoom: Float
    var onRendered: () -> Void

    func makeNSView(context: Context) -> NSImageView {
        let view = NSImageView()
        view.imageScaling = .scaleProportionallyUpOrDown
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        return view
    }

    func updateNSView(_ view: NSImageView, context: Context) {
        guard let document else {
            view.image = nil
            return
        }
        DispatchQueue.global(qos: .utility).async {
            guard let tile = try? document.renderTile(page: page.index, zoom: zoom, tx: 0, ty: 0),
                  let image = cgImage(from: tile)
            else { return }
            let ns = NSImage(cgImage: image, size: NSSize(width: CGFloat(tile.width), height: CGFloat(tile.height)))
            DispatchQueue.main.async {
                view.image = ns
                onRendered()
            }
        }
    }
}
