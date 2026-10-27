import Darwin
import Foundation
import MailroomNative

// Owner commands print names and status codes only. The pipe mode serves the pinned parent.
func owner(_ arguments: [String]) -> Int32 {
    guard arguments.count >= 1 else { return 2 }
    let reference = arguments.count == 2 ? arguments[1] : ""
    guard let keychain = try? Keychain(path: nil, interactive: true) else { return 1 }
    switch (arguments[0], arguments.count) {
    case ("legacy", 1):
        let (status, references) = keychain.legacyReferences()
        references.forEach { print($0) }
        return status == .ok ? 0 : 1
    case ("status", 2) where validReference(reference):
        let (status, protection) = keychain.protection(reference: reference)
        print("status=\(status.rawValue) protection=\(protection?.rawValue ?? "unexpected") expected=\(Protection(reference: reference).rawValue)")
        return status == .ok && protection == Protection(reference: reference) ? 0 : 1
    case ("enroll", 2) where validReference(reference):
        guard isatty(STDIN_FILENO) == 1, var first = hidden("Password for \(reference): ") else { return 1 }
        defer { first.resetBytes(in: 0..<first.count) }
        guard var second = hidden("Repeat it: ") else { return 1 }
        defer { second.resetBytes(in: 0..<second.count) }
        guard first == second else { print("status=mismatch"); return 1 }
        let status = keychain.put(reference: reference, secret: first)
        print("status=\(status.rawValue)")
        return status == .ok ? 0 : 1
    case ("migrate", 2) where validReference(reference):
        var (status, value) = keychain.legacyGet(reference: reference)
        defer { if value != nil { value!.resetBytes(in: 0..<value!.count) } }
        guard status == .ok, let secret = value else { print("status=\(status.rawValue)"); return 1 }
        status = keychain.put(reference: reference, secret: secret)
        print("status=\(status.rawValue)")
        return status == .ok ? 0 : 1
    case ("retire-legacy", 2) where validReference(reference):
        guard keychain.protection(reference: reference) == (.ok, Protection(reference: reference)) else { print("status=protected-item-missing"); return 1 }
        let status = keychain.legacyDelete(reference: reference)
        print("status=\(status.rawValue)")
        return status == .ok || status == .missing ? 0 : 1
    default:
        return 2
    }
}

func hidden(_ prompt: String) -> Data? {
    var buffer = [CChar](repeating: 0, count: 1024)
    defer { _ = memset_s(&buffer, buffer.count, 0, buffer.count) }
    guard readpassphrase(prompt, &buffer, buffer.count, RPP_REQUIRE_TTY) != nil else { return nil }
    let length = strlen(buffer)
    guard length > 0, length < buffer.count - 1 else { return nil }
    return buffer.withUnsafeBytes { Data($0.prefix(length)) }
}

let arguments = Array(CommandLine.arguments.dropFirst())
if !arguments.isEmpty { exit(owner(arguments)) }
do {
    try requirePrivatePipes()
    var input = try readRequest()
    defer { input.resetBytes(in: 0..<input.count) }
    var output = try serve(input)
    write(output)
    output.resetBytes(in: 0..<output.count)
} catch {
    exit(1)
}
