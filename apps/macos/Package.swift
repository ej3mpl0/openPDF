// SPDX-License-Identifier: MPL-2.0
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "OpenPDF",
    defaultLocalization: "en",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "openpdf-macos", targets: ["OpenPDFApp"]),
    ],
    targets: [
        .systemLibrary(
            name: "openpdfFFI",
            path: "Sources/openpdfFFI"
        ),
        .executableTarget(
            name: "OpenPDFApp",
            dependencies: ["openpdfFFI"],
            path: "Sources/OpenPDFApp",
            resources: [
                .process("Resources"),
            ],
            linkerSettings: [
                .linkedLibrary("openpdf_ffi"),
                .linkedFramework("AppKit"),
                .linkedFramework("SwiftUI"),
                .linkedFramework("QuartzCore"),
            ]
        ),
    ]
)
