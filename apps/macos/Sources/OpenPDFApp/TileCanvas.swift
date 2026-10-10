import AppKit
import QuartzCore
import SwiftUI

struct TileCoord: Hashable {
    let page: UInt32
    let zoom: Float
    let tx: UInt32
    let ty: UInt32
}

/// CALayer-backed page stack. Tiles come from core (`renderTile` + LRU cache).
final class TileCanvasView: NSView {
    override var isFlipped: Bool { true }
    override var wantsUpdateLayer: Bool { true }

    var onVisibleTiles: ((Int, Int) -> Void)?

    private var document: PdfDocument?
    private var pages: [PageInfo] = []
    private var zoom: Float = 1.0
    private var pageOrigin: [CGPoint] = []
    private var tileLayers: [TileCoord: CALayer] = [:]
    private var inflight: Set<TileCoord> = []
    private let pageGap: CGFloat = 12
    private let contentWidth: CGFloat = 16

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer = CALayer()
        layer?.isOpaque = true
        layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:)") }

    func setDocument(_ document: PdfDocument?, pages: [PageInfo], zoom: Float) {
        self.document = document
        self.pages = pages
        self.zoom = zoom
        tileLayers.values.forEach { $0.removeFromSuperlayer() }
        tileLayers.removeAll()
        inflight.removeAll()
        relayout()
        loadVisible()
    }

    func setZoom(_ zoom: Float) {
        guard zoom != self.zoom else { return }
        self.zoom = zoom
        tileLayers.values.forEach { $0.removeFromSuperlayer() }
        tileLayers.removeAll()
        inflight.removeAll()
        relayout()
        loadVisible()
    }

    func pageOrigin(at index: Int) -> CGPoint? {
        guard pages.indices.contains(index) else { return nil }
        return pageOrigin[index]
    }

    override func layout() {
        super.layout()
        relayout()
        loadVisible()
    }

    override func viewDidEndLiveResize() {
        super.viewDidEndLiveResize()
        loadVisible()
    }

    private func relayout() {
        var y: CGFloat = pageGap
        var maxW: CGFloat = 0
        pageOrigin = []
        for page in pages {
            let w = CGFloat(page.widthPt * zoom)
            let h = CGFloat(page.heightPt * zoom)
            pageOrigin.append(CGPoint(x: contentWidth, y: y))
            maxW = max(maxW, w)
            y += h + pageGap
        }
        let size = NSSize(width: max(maxW + contentWidth * 2, bounds.width), height: y)
        if frame.size != size {
            setFrameSize(size)
        }
    }

    func loadVisible() {
        guard let document else {
            onVisibleTiles?(0, 0)
            return
        }
        let visible = visibleRect
        let keys = visibleKeys(in: visible)
        onVisibleTiles?(keys.filter { tileLayers[$0] != nil }.count, keys.count)
        for key in keys where tileLayers[key] == nil && !inflight.contains(key) {
            inflight.insert(key)
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let tile = try? document.renderTile(page: key.page, zoom: key.zoom, tx: key.tx, ty: key.ty)
                DispatchQueue.main.async {
                    guard let self else { return }
                    self.inflight.remove(key)
                    if let tile, let image = cgImage(from: tile) {
                        self.attach(key: key, image: image, tile: tile)
                    }
                    let ready = keys.filter { self.tileLayers[$0] != nil }.count
                    self.onVisibleTiles?(ready, keys.count)
                }
            }
        }
    }

    func renderKeysNow(_ keys: [TileCoord]) -> (ready: Int, total: Int) {
        guard let document else { return (0, 0) }
        var ready = 0
        for key in keys {
            if tileLayers[key] != nil {
                ready += 1
                continue
            }
            guard let tile = try? document.renderTile(
                page: key.page, zoom: key.zoom, tx: key.tx, ty: key.ty
            ), let image = cgImage(from: tile) else { continue }
            attach(key: key, image: image, tile: tile)
            ready += 1
        }
        return (ready, keys.count)
    }

    func keysForPage(_ page: PageInfo, zoom: Float, viewport: CGRect) -> [TileCoord] {
        guard pages.indices.contains(Int(page.index)) else { return [] }
        return keys(for: page, zoom: zoom, visible: viewport, origin: pageOrigin[Int(page.index)])
    }

    private func visibleKeys(in visible: CGRect) -> [TileCoord] {
        var out: [TileCoord] = []
        for (i, page) in pages.enumerated() {
            out.append(contentsOf: keys(for: page, zoom: zoom, visible: visible, origin: pageOrigin[i]))
        }
        return out
    }

    private func keys(for page: PageInfo, zoom: Float, visible: CGRect, origin: CGPoint) -> [TileCoord] {
        let ts = CGFloat(tileSize())
        let pw = CGFloat(page.widthPt * zoom)
        let ph = CGFloat(page.heightPt * zoom)
        let pageRect = CGRect(origin: origin, size: CGSize(width: pw, height: ph))
        let clip = visible.intersection(pageRect)
        guard !clip.isNull, !clip.isEmpty else { return [] }
        let local = clip.offsetBy(dx: -origin.x, dy: -origin.y)
        let maxTx = max(0, Int(ceil(pw / ts)) - 1)
        let maxTy = max(0, Int(ceil(ph / ts)) - 1)
        let x0 = min(maxTx, max(0, Int(floor(local.minX / ts))))
        let y0 = min(maxTy, max(0, Int(floor(local.minY / ts))))
        let x1 = min(maxTx, max(0, Int(floor((max(local.maxX - 1, local.minX)) / ts))))
        let y1 = min(maxTy, max(0, Int(floor((max(local.maxY - 1, local.minY)) / ts))))
        var keys: [TileCoord] = []
        if x0 <= x1, y0 <= y1 {
            for ty in y0 ... y1 {
                for tx in x0 ... x1 {
                    keys.append(TileCoord(page: page.index, zoom: zoom, tx: UInt32(tx), ty: UInt32(ty)))
                }
            }
        }
        return keys
    }

    private func attach(key: TileCoord, image: CGImage, tile: RasterTile) {
        let layer = CALayer()
        layer.contents = image
        layer.contentsGravity = .resize
        layer.isOpaque = true
        let ts = CGFloat(tileSize())
        guard pages.indices.contains(Int(key.page)) else { return }
        let origin = pageOrigin[Int(key.page)]
        layer.frame = CGRect(
            x: origin.x + CGFloat(key.tx) * ts,
            y: origin.y + CGFloat(key.ty) * ts,
            width: CGFloat(tile.width),
            height: CGFloat(tile.height)
        )
        self.layer?.addSublayer(layer)
        tileLayers[key] = layer
    }
}

func cgImage(from tile: RasterTile) -> CGImage? {
    let width = Int(tile.width)
    let height = Int(tile.height)
    guard width > 0, height > 0, tile.rgba.count >= width * height * 4 else { return nil }
    let data = tile.rgba as CFData
    guard let provider = CGDataProvider(data: data) else { return nil }
    return CGImage(
        width: width,
        height: height,
        bitsPerComponent: 8,
        bitsPerPixel: 32,
        bytesPerRow: width * 4,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
        provider: provider,
        decode: nil,
        shouldInterpolate: false,
        intent: .defaultIntent
    )
}

struct TileCanvasRepresentable: NSViewRepresentable {
    var document: PdfDocument?
    var pages: [PageInfo]
    var zoom: Float
    var onVisibleTiles: (Int, Int) -> Void

    func makeNSView(context: Context) -> TileCanvasView {
        let view = TileCanvasView(frame: .zero)
        view.onVisibleTiles = onVisibleTiles
        return view
    }

    func updateNSView(_ view: TileCanvasView, context: Context) {
        view.onVisibleTiles = onVisibleTiles
        let token = document.map { ObjectIdentifier($0) } ?? ObjectIdentifier(NSNull())
        if token != context.coordinator.token || pages.count != context.coordinator.pageCount {
            context.coordinator.token = token
            context.coordinator.pageCount = pages.count
            view.setDocument(document, pages: pages, zoom: zoom)
        } else {
            view.setZoom(zoom)
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator {
        var pageCount = -1
        var token = ObjectIdentifier(NSNull())
    }
}
