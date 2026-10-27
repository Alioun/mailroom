import CryptoKit
import Foundation

public struct Denied: Error, Equatable {
    public init() {}
}

/// One server-owned action. Fields mirror `confirmation::ExactAction` in the Rust core.
public struct ExactAction: Decodable, Sendable {
    let purpose: String
    let account: String
    let operation: String?
    let manifest_digest: String?
    let consequences: String?
    let target_name: String?
    let target: String?
    let sending_identity: String?
    let receiving_identity: String?
    let subject: String?
    let content_digest: String?
    let subscription: String?
    let method: String?
    let host: String?
    let effect: String?
    let plan_receipt: String?

    func required(_ value: String?) throws -> String {
        guard let value else { throw Denied() }
        return value
    }

    /// The exact sentence the Rust core derives for the same action.
    public func display() throws -> String {
        switch purpose {
        case "recovery":
            return "\(try required(operation)) for account \(account). Exact recovery plan SHA-256 \(try required(manifest_digest)). \(try required(consequences))"
        case "invoice_send":
            return "Send this invoice for account \(account) to \(try required(target_name)) <\(try required(target))> from \(try required(sending_identity)). Exact content SHA-256 \(try required(content_digest)). No Undo."
        case "unsubscribe_local_mail":
            return "Send this unsubscribe message for account \(account) to \(try required(target)) from \(try required(sending_identity)), received at \(try required(receiving_identity)). Subject \(try required(subject)). Exact content SHA-256 \(try required(content_digest)). Success unknown. No Undo."
        case "mailbox_change":
            return "\(try required(operation)) for account \(account). Exact previewed plan receipt \(try required(plan_receipt)). Mailbox changes reach the provider."
        case "subscription_action":
            let method = try required(method)
            guard method.allSatisfy({ $0.isASCII && ($0.isLowercase || $0 == "_") }) else { throw Denied() }
            return "Apply synthetic \"\(method)\" for subscription \(try required(subscription)), account \(account), received at \(try required(receiving_identity)), host \(host ?? "none"). Payload SHA-256 \(try required(content_digest)). \(try required(effect))"
        default:
            throw Denied()
        }
    }
}

public struct ExactRequest: Decodable, Sendable {
    public let request_id: String
    let `protocol`: String
    public let actions: [ExactAction]
    let plan_digest: String
    public let display: String
    let expires_after_seconds: UInt64
    let synthetic: Bool
}

public enum Limits {
    public static let envelope = 65536
    public static let request = 49152
    public static let display = 8192
    public static let actions = 64
}

func isDigest(_ value: String) -> Bool {
    value.utf8.count == 64 && value.utf8.allSatisfy { (0x30...0x39).contains($0) || (0x61...0x66).contains($0) }
}

public func hex<D: Sequence>(_ bytes: D) -> String where D.Element == UInt8 {
    bytes.map { String(format: "%02x", $0) }.joined()
}

/// Parses the exact request bytes and rederives the display. Anything unexpected denies.
public func validate(request bytes: Data) throws -> ExactRequest {
    guard bytes.count <= Limits.request else { throw Denied() }
    let request = try JSONDecoder().decode(ExactRequest.self, from: bytes)
    guard request.protocol == "mailroom-exact-action-v1", !request.synthetic,
          isDigest(request.request_id), isDigest(request.plan_digest),
          (1...Limits.actions).contains(request.actions.count),
          request.expires_after_seconds == 300 else { throw Denied() }
    let display = try request.actions.map { try $0.display() }.joined(separator: "\n")
    guard display == request.display, display.utf8.count <= Limits.display else { throw Denied() }
    return request
}

public enum Outcome: String, Sendable {
    case approved
    case cancelled
}

struct WireOutcome: Encodable {
    let request_id: String
    let request_digest: String
    let outcome: String
    let authentication: String
}

/// The outcome is bound to the request bytes and keyed by the parent's one-time challenge.
public func outcome(request bytes: Data, id: String, outcome: Outcome, challenge: Data) throws -> Data {
    guard challenge.count == 32, isDigest(id) else { throw Denied() }
    let digest = hex(SHA256.hash(data: bytes))
    var message = Data("mailroom-exact-action-outcome-v1\0".utf8)
    message.append(Data("[\"\(id)\",\"\(digest)\",\"\(outcome.rawValue)\"]".utf8))
    let tag = hex(HMAC<SHA256>.authenticationCode(for: message, using: SymmetricKey(data: challenge)))
    return try JSONEncoder().encode(WireOutcome(request_id: id, request_digest: digest, outcome: outcome.rawValue, authentication: tag))
}

public struct Envelope: Decodable {
    public var request: Data
    public var challenge: Data

    /// Overwrites both buffers in place. Call it once no other copy is alive, so the
    /// buffers are uniquely owned and copy-on-write cannot redirect the reset.
    public mutating func wipe() {
        challenge.withUnsafeMutableBytes { $0.initializeMemory(as: UInt8.self, repeating: 0) }
        request.withUnsafeMutableBytes { $0.initializeMemory(as: UInt8.self, repeating: 0) }
    }
}
