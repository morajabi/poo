// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "Poo",
    platforms: [.macOS(.v15)],
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.6.0"),
    ],
    targets: [
        .executableTarget(
            name: "Poo",
            dependencies: [.product(name: "Sparkle", package: "Sparkle")],
            path: "Sources/Poo",
            swiftSettings: [.unsafeFlags(["-parse-as-library"])]
        ),
        .testTarget(
            name: "PooTests",
            dependencies: ["Poo"],
            path: "Tests/PooTests"
        ),
    ],
    swiftLanguageModes: [.v5]
)
