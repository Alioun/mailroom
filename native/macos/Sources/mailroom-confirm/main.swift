import AppKit
import Foundation
import LocalAuthentication
import MailroomNative

/// Shows the full exact action, then requires the device owner. Either refusal cancels.
@MainActor
func ask(_ request: ExactRequest) async -> Bool {
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
    return await withCheckedContinuation { continuation in
        context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) { success, _ in
            continuation.resume(returning: success)
        }
    }
}

do {
    try requirePrivatePipes()
    let envelope = try JSONDecoder().decode(Envelope.self, from: try readRequest())
    let request = try validate(request: envelope.request)
    let approved = await ask(request)
    write(try outcome(request: envelope.request, id: request.request_id, outcome: approved ? .approved : .cancelled, challenge: envelope.challenge))
} catch {
    // No parser data, challenge or OS error text leaves the helper.
    exit(1)
}
