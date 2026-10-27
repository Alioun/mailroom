import CryptoKit
import Foundation

public enum Operation: String, Decodable, Sendable {
    case get, put, delete, status
}

/// One request from the pinned parent over the private stdin pipe.
public struct KeychainRequest: Decodable {
    public let op: Operation
    public let keychain: String?
    public let interactive: Bool
    public let reference: String
    public let secret: Data?
    public let challenge: Data
}

struct KeychainResponse: Encodable {
    let status: Status
    let secret: Data?
    let protection: Protection?
    let authentication: String
}

/// Binds the reply to this request's operation, reference and one-time challenge.
public func channelTag(challenge: Data, op: Operation, reference: String, status: Status, protection: Protection?, secret: Data?) -> String {
    var message = Data("mailroom-keychain-v1\0\(op.rawValue)\0\(reference)\0\(status.rawValue)\0\(protection?.rawValue ?? "")\0".utf8)
    message.append(Data(hex(SHA256.hash(data: secret ?? Data())).utf8))
    return hex(HMAC<SHA256>.authenticationCode(for: message, using: SymmetricKey(data: challenge)))
}

/// Serves one request. Errors become fixed status codes; the secret appears only in a successful get.
public func serve(_ input: Data) throws -> Data {
    let request = try JSONDecoder().decode(KeychainRequest.self, from: input)
    guard request.challenge.count == 32, validReference(request.reference),
          (request.op == .put) == (request.secret != nil) else { throw Denied() }
    let keychain = try Keychain(path: request.keychain, interactive: request.interactive)
    var status: Status
    var secret: Data?
    var protection: Protection?
    switch request.op {
    case .get:
        (status, secret) = keychain.get(reference: request.reference)
    case .put:
        status = keychain.put(reference: request.reference, secret: request.secret ?? Data())
    case .delete:
        status = keychain.delete(reference: request.reference)
    case .status:
        (status, protection) = keychain.protection(reference: request.reference)
    }
    let tag = channelTag(challenge: request.challenge, op: request.op, reference: request.reference, status: status, protection: protection, secret: secret)
    let encoder = JSONEncoder()
    encoder.outputFormatting = .withoutEscapingSlashes
    return try encoder.encode(KeychainResponse(status: status, secret: secret, protection: protection, authentication: tag))
}
