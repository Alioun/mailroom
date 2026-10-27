import CryptoKit
import Foundation
import Security
import XCTest
@testable import MailroomNative

/// Every test works in a keychain it creates in a temporary directory and deletes afterwards.
/// User interaction stays disabled, so a read that would prompt fails instead.
final class KeychainTests: XCTestCase {
    var directory: URL!
    var path: String!
    var throwaway: SecKeychain?
    var searchList: [String] = []
    let dummy = Data("nk-dummy-secret-\(UUID().uuidString)".utf8)

    static func searchListPaths() -> [String] {
        var list: CFArray?
        XCTAssertEqual(SecKeychainCopySearchList(&list), errSecSuccess)
        return ((list as? [SecKeychain]) ?? []).map { keychain in
            var length: UInt32 = 1024
            var buffer = [CChar](repeating: 0, count: 1024)
            _ = SecKeychainGetPath(keychain, &length, &buffer)
            return String(cString: buffer)
        }
    }

    override func setUpWithError() throws {
        searchList = Self.searchListPaths()
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("mailroom-keychain-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        path = directory.appendingPathComponent("throwaway.keychain-db").path
        let password = UUID().uuidString
        XCTAssertEqual(SecKeychainCreate(path, UInt32(password.utf8.count), password, false, nil, &throwaway), errSecSuccess)
        XCTAssertEqual(SecKeychainUnlock(throwaway, UInt32(password.utf8.count), password, true), errSecSuccess)
    }

    override func tearDownWithError() throws {
        if let throwaway { XCTAssertEqual(SecKeychainDelete(throwaway), errSecSuccess) }
        try? FileManager.default.removeItem(at: directory)
        XCTAssertEqual(SecKeychainSetUserInteractionAllowed(true), errSecSuccess)
        XCTAssertEqual(Self.searchListPaths(), searchList, "the user's keychain search list is unchanged")
    }

    func keychain() throws -> Keychain {
        try Keychain(path: path, interactive: false)
    }

    func testPinnedItemRoundTripsNeverOverwritesAndDeletesIdempotently() throws {
        let store = try keychain()
        XCTAssertEqual(store.put(reference: "acct-census-key", secret: dummy), .ok)
        XCTAssertEqual(store.put(reference: "acct-census-key", secret: Data("other".utf8)), .exists)
        let (status, value) = store.get(reference: "acct-census-key")
        XCTAssertEqual(status, .ok)
        XCTAssertEqual(value, dummy)
        XCTAssertTrue(store.protection(reference: "acct-census-key") == (.ok, .pinnedHelper))
        XCTAssertEqual(store.delete(reference: "acct-census-key"), .ok)
        XCTAssertTrue(store.get(reference: "acct-census-key") == (.missing, nil))
        XCTAssertEqual(store.delete(reference: "acct-census-key"), .missing)
    }

    func testPasswordItemTrustsNoApplicationAndEveryReadNeedsAPerson() throws {
        let store = try keychain()
        XCTAssertEqual(store.put(reference: "acct-password", secret: dummy), .ok)
        XCTAssertTrue(store.protection(reference: "acct-password") == (.ok, .everyAccess))
        let (status, value) = store.get(reference: "acct-password")
        XCTAssertEqual(status, .denied)
        XCTAssertNil(value)
        XCTAssertEqual(store.delete(reference: "acct-password"), .ok)
    }

    func testItemsLandOnlyInTheThrowawayKeychainAndItsFileHoldsNoPlaintext() throws {
        let store = try keychain()
        XCTAssertEqual(store.put(reference: "acct-key-g2", secret: dummy), .ok)
        let file = try Data(contentsOf: URL(fileURLWithPath: path))
        XCTAssertNil(file.range(of: dummy))
        let other = FileManager.default.temporaryDirectory.appendingPathComponent("mailroom-other-\(UUID().uuidString).keychain-db").path
        var second: SecKeychain?
        XCTAssertEqual(SecKeychainCreate(other, 5, "dummy", false, nil, &second), errSecSuccess)
        defer { if let second { SecKeychainDelete(second) } }
        XCTAssertTrue(try Keychain(path: other, interactive: false).get(reference: "acct-key-g2") == (.missing, nil))
    }

    func testLegacyItemsAreListedByNameOnly() throws {
        let add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Keychain.legacyService,
            kSecAttrAccount as String: "acct-password",
            kSecValueData as String: dummy,
            kSecUseKeychain as String: throwaway!,
        ]
        XCTAssertEqual(SecItemAdd(add as CFDictionary, nil), errSecSuccess)
        let store = try keychain()
        let (status, references) = store.legacyReferences()
        XCTAssertEqual(status, .ok)
        XCTAssertEqual(references, ["acct-password"])
        XCTAssertTrue(store.protection(reference: "acct-password") == (.missing, nil))
        XCTAssertEqual(store.legacyDelete(reference: "acct-password"), .ok)
        XCTAssertTrue(store.legacyReferences() == (.ok, []))
    }

    func channel(_ object: [String: Any]) throws -> [String: Any] {
        let output = try serve(JSONSerialization.data(withJSONObject: object))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: output) as? [String: Any])
    }

    func testPipeProtocolAuthenticatesEveryReplyAndReturnsFixedCodes() throws {
        let challenge = Data((0..<32).map { _ in UInt8.random(in: 0...255) })
        var request: [String: Any] = ["op": "put", "keychain": path!, "interactive": false, "reference": "acct-census-key", "secret": dummy.base64EncodedString(), "challenge": challenge.base64EncodedString()]
        XCTAssertEqual(try channel(request)["status"] as? String, "ok")
        request["op"] = "get"
        request.removeValue(forKey: "secret")
        let got = try channel(request)
        XCTAssertEqual(got["secret"] as? String, dummy.base64EncodedString())
        XCTAssertEqual(got["authentication"] as? String, channelTag(challenge: challenge, op: .get, reference: "acct-census-key", status: .ok, protection: nil, secret: dummy))
        request["op"] = "status"
        XCTAssertEqual(try channel(request)["protection"] as? String, "pinned_helper")
        request["reference"] = "acct-missing-password"
        request["op"] = "get"
        let missing = try channel(request)
        XCTAssertEqual(missing["status"] as? String, "missing")
        XCTAssertNil(missing["secret"])
        for bad: [String: Any] in [
            ["op": "get", "keychain": path!, "interactive": false, "reference": "../x", "challenge": challenge.base64EncodedString()],
            ["op": "get", "keychain": path!, "interactive": false, "reference": "acct-password", "secret": dummy.base64EncodedString(), "challenge": challenge.base64EncodedString()],
            ["op": "put", "keychain": path!, "interactive": false, "reference": "acct-password", "challenge": challenge.base64EncodedString()],
            ["op": "get", "keychain": path!, "interactive": false, "reference": "acct-password", "challenge": Data(count: 8).base64EncodedString()],
            ["op": "get", "keychain": "relative.keychain-db", "interactive": false, "reference": "acct-password", "challenge": challenge.base64EncodedString()],
            ["op": "export", "keychain": path!, "interactive": false, "reference": "acct-password", "challenge": challenge.base64EncodedString()],
        ] {
            XCTAssertThrowsError(try serve(JSONSerialization.data(withJSONObject: bad)))
        }
    }
}
