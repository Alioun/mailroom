// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "MailroomNative",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "mailroom-confirm", targets: ["mailroom-confirm"]),
    ],
    targets: [
        .target(name: "MailroomNative"),
        .executableTarget(name: "mailroom-confirm", dependencies: ["MailroomNative"]),
        .testTarget(
            name: "MailroomNativeTests",
            dependencies: ["MailroomNative"],
            resources: [.copy("exact-actions.json")]
        ),
    ]
)
