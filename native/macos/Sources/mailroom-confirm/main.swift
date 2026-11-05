import AppKit
import Foundation
import LocalAuthentication
import MailroomNative

// The semaphore publishes success before the main thread reads it.
final class AuthenticationReply: @unchecked Sendable {
    let done = DispatchSemaphore(value: 0)
    var success = false
}

/// Shows the full exact action, then requires the device owner. Either refusal cancels.
// A top-level await lets Swift exit when AppKit stops the main run loop,
// even while authentication is pending. Keep this call synchronous until the reply.
@MainActor
func ask(_ request: ExactRequest) -> Bool {
    NSApplication.shared.setActivationPolicy(.accessory)
    NSApplication.shared.activate()
    let alert = NSAlert()
    alert.alertStyle = .warning
    alert.messageText = "Mailroom asks you to approve exactly this"
    alert.informativeText = request.display
    alert.addButton(withTitle: "Cancel")
    alert.addButton(withTitle: "Approve")
    guard alert.runModal() == .alertSecondButtonReturn else { return false }
    let context = LAContext()
    context.touchIDAuthenticationAllowableReuseDuration = 0
    defer { context.invalidate() }
    var error: NSError?
    guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else { return false }
    let first = request.display.split(separator: "\n").first.map(String.init) ?? ""
    let reason = request.actions.count == 1 ? first : "\(first) (and \(request.actions.count - 1) more listed actions)"
    let reply = AuthenticationReply()
    context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) { success, _ in
        reply.success = success
        reply.done.signal()
    }
    while reply.done.wait(timeout: .now() + .milliseconds(50)) == .timedOut {
        _ = RunLoop.main.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
    }
    return reply.success
}

do {
    try disableCoreDumps()
    try requirePrivatePipes()
    var input = try readRequest()
    defer { input.resetBytes(in: 0..<input.count) }
    var envelope = try JSONDecoder().decode(Envelope.self, from: input)
    defer { envelope.wipe() }
    let request = try validate(request: envelope.request)
    let approved = ask(request)
    let answer = try outcome(request: envelope.request, id: request.request_id, outcome: approved ? .approved : .cancelled, challenge: envelope.challenge)
    write(answer)
} catch {
    // No parser data, challenge or OS error text leaves the helper.
    exit(1)
}
