// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "MailroomNative",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "mailroom-confirm", targets: ["mailroom-confirm"]),
        .executable(name: "mailroom-keychain", targets: ["mailroom-keychain"]),
    ],
    targets: [
        .target(name: "MailroomNative"),
        .executableTarget(name: "mailroom-confirm", dependencies: ["MailroomNative"]),
        .executableTarget(name: "mailroom-keychain", dependencies: ["MailroomNative"]),
        .testTarget(
            name: "MailroomNativeTests",
            dependencies: ["MailroomNative"],
            resources: [.copy("exact-actions.json")]
        ),
    ]
)
