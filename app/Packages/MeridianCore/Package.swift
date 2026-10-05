// swift-tools-version: 6.0
// Wraps the Rust core: the XCFramework produced by scripts/build-core.sh and
// the UniFFI-generated Swift bindings. Generated code compiles in Swift 5
// mode with nonisolated defaults (UniFFI's async glue is not fully Sendable).
import PackageDescription

let package = Package(
    name: "MeridianCore",
    platforms: [.macOS(.v15)],
    products: [.library(name: "MeridianCore", targets: ["MeridianCore"])],
    targets: [
        .binaryTarget(name: "MeridianCoreFFI", path: "MeridianCoreFFI.xcframework"),
        .target(
            name: "MeridianCore",
            dependencies: ["MeridianCoreFFI"],
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                .linkedLibrary("c++"),
                .linkedLibrary("iconv"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
