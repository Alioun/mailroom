import CryptoKit
import Darwin
import Foundation
import LocalAuthentication
import Security

// Provisioned requirements must be compiled into a signed owner-controlled build.
enum OwnerTrust {
    static let helperRequirement: String? = nil
    static let applicationRequirement: String? = nil
}

enum HelperError: Error { case denied }

struct Action: Decodable {
    let purpose: String
    let account: String
    let revision: UInt64
    let target: String
    let content_digest: String
    let target_name: String?
    let sending_identity: String
    let receiving_identity: String?
    let subject: String?

    func display() throws -> String {
        switch purpose {
        case "invoice_send":
            guard let name = target_name else { throw HelperError.denied }
            return "Send this invoice for account \(account) to \(name) <\(target)> from \(sending_identity). Exact content SHA-256 \(content_digest). No Undo."
        case "unsubscribe_local_mail":
            guard let recipient = receiving_identity, let subject else { throw HelperError.denied }
            return "Send this unsubscribe message for account \(account) to \(target) from \(sending_identity), received at \(recipient). Subject \(subject). Exact content SHA-256 \(content_digest). Success unknown. No Undo."
        default: throw HelperError.denied
        }
    }
}
struct ExactRequest: Decodable {
    let request_id: String
    let `protocol`: String
    let actions: [Action]
    let plan_digest: String
    let display: String
    let expires_after_seconds: UInt64
    let synthetic: Bool
}
struct Envelope: Decodable {
    let request: Data
    let challenge: Data
}
struct WireOutcome: Encodable {
    let request_id: String
    let request_digest: String
    let outcome: String
    let authentication: String
}

func requirement(_ source: String?) throws -> SecRequirement {
    guard let source, !source.isEmpty else { throw HelperError.denied }
    var result: SecRequirement?
    guard SecRequirementCreateWithString(source as CFString, [], &result) == errSecSuccess,
          let result else { throw HelperError.denied }
    return result
}

func verifyRunningIdentities() throws {
    let ownRequirement = try requirement(OwnerTrust.helperRequirement)
    let parentRequirement = try requirement(OwnerTrust.applicationRequirement)
    var own: SecCode?
    var parent: SecCode?
    guard SecCodeCopySelf([], &own) == errSecSuccess, let own,
          SecCodeCheckValidity(own, [], ownRequirement) == errSecSuccess,
          SecCodeCopyGuestWithAttributes(nil, [kSecGuestAttributePid: getppid()] as CFDictionary, [], &parent) == errSecSuccess,
          let parent, SecCodeCheckValidity(parent, [], parentRequirement) == errSecSuccess else {
        throw HelperError.denied
    }
}

// Static verification requires an owner-selected requirement, never any valid signer.
func verifyStaticHelper(at url: URL, expected: String) throws {
    let expectedRequirement = try requirement(expected)
    var code: SecStaticCode?
    guard SecStaticCodeCreateWithPath(url as CFURL, [], &code) == errSecSuccess, let code,
          SecStaticCodeCheckValidityWithErrors(code, SecCSFlags(rawValue: kSecCSCheckAllArchitectures | kSecCSStrictValidate), expectedRequirement, nil) == errSecSuccess else {
        throw HelperError.denied
    }
}

// This constructs a constraint only. It neither accesses nor installs a Keychain item.
func credentialPresenceConstraint() throws -> SecAccessControl {
    var error: Unmanaged<CFError>?
    guard let control = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, .userPresence, &error) else {
        throw HelperError.denied
    }
    return control
}

@main struct ConfirmationHelper {
    static func main() async {
        do {
            guard CommandLine.arguments.count == 1, isatty(STDIN_FILENO) == 0, isatty(STDOUT_FILENO) == 0 else { throw HelperError.denied }
            var inputStat = stat()
            var outputStat = stat()
            guard fstat(STDIN_FILENO, &inputStat) == 0, fstat(STDOUT_FILENO, &outputStat) == 0,
                  inputStat.st_mode & S_IFMT == S_IFIFO, outputStat.st_mode & S_IFMT == S_IFIFO else { throw HelperError.denied }
            try verifyRunningIdentities()
            guard let input = try FileHandle.standardInput.read(upToCount: 65537), input.count <= 65536 else { throw HelperError.denied }
            let envelope = try JSONDecoder().decode(Envelope.self, from: input)
            guard envelope.challenge.count == 32, envelope.request.count <= 49152 else { throw HelperError.denied }
            let request = try JSONDecoder().decode(ExactRequest.self, from: envelope.request)
            guard request.protocol == "mailroom-exact-action-v1", !request.synthetic,
                  request.request_id.count == 64, request.plan_digest.count == 64,
                  request.actions.count == 1,
                  request.expires_after_seconds == 300 else { throw HelperError.denied }
            let display = try request.actions.map { try $0.display() }.joined(separator: "\n")
            guard display == request.display, display.utf8.count <= 1024 else { throw HelperError.denied }
            let context = LAContext()
            context.touchIDAuthenticationAllowableReuseDuration = 0
            defer { context.invalidate() }
            var policyError: NSError?
            guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &policyError) else { throw HelperError.denied }
            let approved = await withCheckedContinuation { continuation in
                context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: display) { success, _ in
                    continuation.resume(returning: success)
                }
            }
            let outcome = approved ? "approved" : "cancelled"
            let requestDigest = SHA256.hash(data: envelope.request).map { String(format: "%02x", $0) }.joined()
            let payload = try JSONSerialization.data(withJSONObject: [request.request_id, requestDigest, outcome], options: [.withoutEscapingSlashes])
            var authenticated = Data("mailroom-exact-action-outcome-v1\0".utf8)
            authenticated.append(payload)
            let key = SymmetricKey(data: envelope.challenge)
            let tag = HMAC<SHA256>.authenticationCode(for: authenticated, using: key).map { String(format: "%02x", $0) }.joined()
            let output = WireOutcome(request_id: request.request_id, request_digest: requestDigest, outcome: outcome, authentication: tag)
            FileHandle.standardOutput.write(try JSONEncoder().encode(output))
        } catch {
            // No parser data, challenge, identity or OS error text leaves the helper.
            exit(1)
        }
    }
}
