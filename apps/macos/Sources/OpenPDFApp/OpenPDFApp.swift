import AppKit
import Darwin
import SwiftUI

@main
struct OpenPDFMain {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if let idx = args.firstIndex(of: "--measure") {
            let pages: Int
            if idx + 1 < args.count, let parsed = Int(args[idx + 1]), parsed > 0 {
                pages = parsed
            } else {
                pages = Measure.defaultPages
            }
            Measure.markProcessStart()
            Measure.bootstrapApp()
            exit(Int32(Measure.run(pages: pages)))
        }
        OpenPDFApp.main()
    }
}

struct OpenPDFApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
                .frame(minWidth: 720, minHeight: 480)
        }
        .commands {
            CommandGroup(replacing: .newItem) {
                Button(L("Open")) { model.openPanel() }
                    .keyboardShortcut("o", modifiers: .command)
            }
        }
    }
}
