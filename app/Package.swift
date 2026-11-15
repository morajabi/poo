// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "Poo",
    platforms: [.macOS(.v14)],
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
    ]
)
