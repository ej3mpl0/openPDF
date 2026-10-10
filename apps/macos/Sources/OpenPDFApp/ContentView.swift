// SPDX-License-Identifier: MPL-2.0
import SwiftUI

struct ContentView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        NavigationSplitView {
            VStack(alignment: .leading, spacing: 0) {
                Text(L("Thumbnails"))
                    .font(.headline)
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                ThumbnailSidebar(
                    document: model.document,
                    pages: model.pages,
                    currentPage: model.currentPage,
                    onSelect: { model.goTo(page: $0) },
                    onReady: { ready, total in
                        model.thumbReady = ready
                        model.thumbTotal = total
                    }
                )
            }
            .frame(minWidth: 140, idealWidth: 168, maxWidth: 220)
        } detail: {
            VStack(spacing: 0) {
                if model.isSigned {
                    Text(L("Signed document — read-only"))
                        .font(.callout)
                        .foregroundStyle(.orange)
                        .frame(maxWidth: .infinity)
                        .padding(8)
                        .background(Color.orange.opacity(0.12))
                }
                if let err = model.errorText {
                    Text(err)
                        .font(.callout)
                        .foregroundStyle(.red)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                }
                toolbar
                ScrollViewReader { proxy in
                    ScrollView([.horizontal, .vertical]) {
                        TileCanvasRepresentable(
                            document: model.document,
                            pages: model.pages,
                            zoom: model.zoom,
                            onVisibleTiles: { ready, total in
                                model.visibleReady = ready
                                model.visibleTotal = total
                            }
                        )
                        .id(model.path ?? "")
                    }
                    .background(Color(nsColor: .windowBackgroundColor))
                    .onChange(of: model.currentPage) { page in
                        proxy.scrollTo(model.path ?? "", anchor: .top)
                        _ = page
                    }
                }
            }
        }
        .navigationTitle(model.path.map { URL(fileURLWithPath: $0).lastPathComponent } ?? L("Open PDF"))
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Button(L("Open")) { model.openPanel() }
            }
        }
    }

    private var toolbar: some View {
        HStack(spacing: 12) {
            Button(L("Previous page")) { model.goTo(page: model.currentPage - 1) }
                .disabled(model.currentPage <= 0)
            Text(
                L("Page {page} of {total}", [
                    "page": String(model.currentPage + 1),
                    "total": String(max(model.pages.count, 0)),
                ])
            )
            Button(L("Next page")) { model.goTo(page: model.currentPage + 1) }
                .disabled(model.currentPage + 1 >= model.pages.count)
            Spacer()
            Button(L("Zoom out")) { model.setZoom(model.zoom / 1.25) }
            Text("\(Int((model.zoom * 100).rounded()))%")
                .monospacedDigit()
            Button(L("Zoom in")) { model.setZoom(model.zoom * 1.25) }
            Button(L("Sign")) { model.signDocument() }
                .disabled(model.document == nil || model.isSigned)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
    }
}
