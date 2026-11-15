// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "Poo",
    platforms: [.macOS(.v15)],
    targets: [
        .executableTarget(
            name: "Poo",
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
