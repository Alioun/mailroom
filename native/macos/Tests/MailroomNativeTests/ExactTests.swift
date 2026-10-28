import CryptoKit
import Foundation
import XCTest
@testable import MailroomNative

struct Fixture: Decodable {
    struct Case: Decodable {
        let action: AnyJSON
        let display: String
    }
    struct OutcomeVector: Decodable {
        let request: String
        let challenge_hex: String
        let request_digest: String
        let approved_tag: String
    }
    let actions: [Case]
    let outcome: OutcomeVector

    static func load() throws -> Fixture {
        let url = try XCTUnwrap(Bundle.module.url(forResource: "exact-actions", withExtension: "json"))
        return try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: url))
    }
}

/// Keeps a fixture value as raw JSON so it can be re-encoded byte-for-byte into a request.
struct AnyJSON: Decodable {
    let value: Any
    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { value = NSNull() }
        else if let v = try? container.decode(Bool.self) { value = v }
        else if let v = try? container.decode(Int.self) { value = v }
        else if let v = try? container.decode(String.self) { value = v }
        else if let v = try? container.decode([String: AnyJSON].self) { value = v.mapValues(\.value) }
        else { value = try container.decode([AnyJSON].self).map(\.value) }
    }
    func data() throws -> Data { try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) }
}

func bytes(_ hex: String) -> Data {
    var data = Data()
    var index = hex.startIndex
    while index < hex.endIndex {
        let next = hex.index(index, offsetBy: 2)
        data.append(UInt8(hex[index..<next], radix: 16)!)
        index = next
    }
    return data
}

func request(_ fixture: Fixture, _ edit: (inout [String: Any]) -> Void = { _ in }) throws -> Data {
    var object = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(fixture.outcome.request.utf8)) as? [String: Any])
    edit(&object)
    return try JSONSerialization.data(withJSONObject: object)
}

final class ExactTests: XCTestCase {
    func testDisplayMatchesTheRustCoreForEveryPurpose() throws {
        let fixture = try Fixture.load()
        XCTAssertEqual(fixture.actions.count, 10)
        for item in fixture.actions {
            let action = try JSONDecoder().decode(ExactAction.self, from: item.action.data())
            XCTAssertEqual(try action.display(), item.display)
        }
    }

    func testExactRequestValidatesAndOutcomeMatchesRustAuthentication() throws {
        let fixture = try Fixture.load()
        let raw = Data(fixture.outcome.request.utf8)
        let parsed = try validate(request: raw)
        let challenge = bytes(fixture.outcome.challenge_hex)
        let wire = try JSONSerialization.jsonObject(with: outcome(request: raw, id: parsed.request_id, outcome: .approved, challenge: challenge)) as? [String: String]
        XCTAssertEqual(wire?["request_digest"], fixture.outcome.request_digest)
        XCTAssertEqual(wire?["authentication"], fixture.outcome.approved_tag)
        XCTAssertEqual(wire?["outcome"], "approved")
        let cancelled = try JSONSerialization.jsonObject(with: outcome(request: raw, id: parsed.request_id, outcome: .cancelled, challenge: challenge)) as? [String: String]
        XCTAssertNotEqual(cancelled?["authentication"], fixture.outcome.approved_tag)
        XCTAssertThrowsError(try outcome(request: raw, id: parsed.request_id, outcome: .approved, challenge: Data(count: 16)))
    }

    func testAlteredSyntheticOversizedAndUnknownRequestsDeny() throws {
        let fixture = try Fixture.load()
        let edits: [(inout [String: Any]) -> Void] = [
            { $0["synthetic"] = true },
            { $0["protocol"] = "mailroom-exact-action-v0" },
            { $0["display"] = ($0["display"] as! String) + " " },
            { $0["expires_after_seconds"] = 301 },
            { $0["request_id"] = String(repeating: "G", count: 64) },
            { $0["plan_digest"] = "short" },
            { $0["actions"] = [] },
            { $0["actions"] = Array(repeating: ($0["actions"] as! [Any])[0], count: 65) },
            { var a = ($0["actions"] as! [[String: Any]])[0]; a["target"] = "other@example.test"; $0["actions"] = [a] },
            { var a = ($0["actions"] as! [[String: Any]])[0]; a["purpose"] = "provider_write"; $0["actions"] = [a] },
            { var a = ($0["actions"] as! [[String: Any]])[0]; a.removeValue(forKey: "sending_identity"); $0["actions"] = [a] },
        ]
        for edit in edits {
            XCTAssertThrowsError(try validate(request: try request(fixture, edit)))
        }
        XCTAssertThrowsError(try validate(request: Data(count: Limits.request + 1)))
        XCTAssertThrowsError(try validate(request: Data("not json".utf8)))
    }

    func testCoreDumpsAreDisabledBeforeAnythingIsRead() throws {
        try disableCoreDumps()
        var limit = rlimit()
        XCTAssertEqual(getrlimit(RLIMIT_CORE, &limit), 0)
        XCTAssertEqual(limit.rlim_cur, 0)
        XCTAssertEqual(limit.rlim_max, 0)
    }

    func testEnvelopeWipeOverwritesItsOwnBuffers() throws {
        var envelope = try JSONDecoder().decode(Envelope.self, from: Data(#"{"request":"AAEC","challenge":"AwQF"}"#.utf8))
        envelope.wipe()
        XCTAssertEqual(envelope.challenge, Data(count: 3))
        XCTAssertEqual(envelope.request, Data(count: 3))
    }
}
