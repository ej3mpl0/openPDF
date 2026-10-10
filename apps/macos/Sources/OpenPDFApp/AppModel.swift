// SPDX-License-Identifier: MPL-2.0
import AppKit
import Combine
import SwiftUI
import UniformTypeIdentifiers

@MainActor
final class AppModel: ObservableObject {
    let library = OpenPdf()
    let pin = MacPinProvider()

    @Published var document: PdfDocument?
    @Published var pages: [PageInfo] = []
    @Published var zoom: Float = 1.0
    @Published var currentPage: Int = 0
    @Published var path: String?
    @Published var isSigned = false
    @Published var errorText: String?
    @Published var visibleReady = 0
    @Published var visibleTotal = 0
    @Published var thumbReady = 0
    @Published var thumbTotal = 0

    func openPanel() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [.pdf]
        panel.title = L("Open PDF")
        if panel.runModal() == .OK, let url = panel.url {
            open(path: url.path)
        }
    }

    func open(path: String) {
        do {
            let doc = try library.open(path: path)
            apply(doc, path: path)
            errorText = nil
        } catch {
            if isSilentPinCancel(error) { return }
            errorText = error.localizedDescription
        }
    }

    func apply(_ doc: PdfDocument, path: String?) {
        document = doc
        self.path = path
        pages = (try? doc.pages()) ?? []
        isSigned = (try? doc.isSigned()) ?? false
        currentPage = 0
        zoom = 1.0
    }

    func setZoom(_ next: Float) {
        zoom = min(4.0, max(0.25, next))
    }

    func goTo(page: Int) {
        guard !pages.isEmpty else { return }
        currentPage = min(max(page, 0), pages.count - 1)
    }

    /// Signed documents stay read-only; mutations would invalidate the signature.
    func guardSigned() -> Bool {
        if isSigned {
            errorText = L(
                "document is signed; {op} would invalidate the signature (incremental save only)",
                ["op": L("edit")]
            )
            return false
        }
        return true
    }

    func signDocument() {
        guard let document else { return }
        guard guardSigned() else { return }
        do {
            let certs = try library.listCertificates(pin: pin)
            guard let cert = certs.first(where: { $0.usages.contains("signing") }) ?? certs.first else {
                errorText = L("no signing certificates found")
                return
            }
            try document.sign(pin: pin, certId: cert.id, reason: nil, location: nil)
            isSigned = (try? document.isSigned()) ?? true
            errorText = nil
        } catch {
            if isSilentPinCancel(error) { return }
            errorText = error.localizedDescription
        }
    }
}
