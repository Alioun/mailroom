import Foundation
import Security

/// How a Keychain item may be read.
public enum Protection: String, Codable, Sendable {
    /// No application is trusted, so the system asks the person at every read.
    case everyAccess = "every_access"
    /// Only this pinned helper reads without a prompt. Any other application triggers a system prompt.
    case pinnedHelper = "pinned_helper"

    /// Local census keys are read on every store open. Mailbox passwords, and any reference not
    /// recognised as a local key, ask a person at every access.
    public init(reference: String) {
        let local = reference.hasSuffix("-census-key") || reference.hasSuffix("-identity")
            || reference.range(of: #"-key-g[0-9]+$"#, options: .regularExpression) != nil
        self = local ? .pinnedHelper : .everyAccess
    }
}

/// Fixed result codes. No OS error text or item data leaves the helper.
public enum Status: String, Codable, Sendable {
    case ok, missing, exists, denied, failed
}

public func validReference(_ reference: String) -> Bool {
    (1...128).contains(reference.utf8.count)
        && reference.utf8.allSatisfy { $0 < 0x80 && (isalnum(Int32($0)) != 0 || $0 == UInt8(ascii: "-")) }
}

public struct Keychain {
    public static let service = "mailroom-protected-v1"
    public static let legacyService = "mailroom"
    let keychain: SecKeychain?

    /// `path` nil means the user's default keychain. Tests always pass a throwaway keychain path.
    /// Disabling interaction makes every read that would prompt fail with `denied` instead.
    public init(path: String?, interactive: Bool) throws {
        if let path {
            var opened: SecKeychain?
            guard path.hasPrefix("/"), SecKeychainOpen(path, &opened) == errSecSuccess, let opened else { throw Denied() }
            keychain = opened
        } else {
            keychain = nil
        }
        guard SecKeychainSetUserInteractionAllowed(interactive) == errSecSuccess else { throw Denied() }
    }

    static func map(_ status: OSStatus) -> Status {
        switch status {
        case errSecSuccess: .ok
        case errSecItemNotFound: .missing
        case errSecDuplicateItem: .exists
        case errSecAuthFailed, errSecInteractionNotAllowed, errSecUserCanceled: .denied
        default: .failed
        }
    }

    func query(_ service: String, _ reference: String?) -> [String: Any] {
        var query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service]
        if let reference { query[kSecAttrAccount as String] = reference }
        if let keychain { query[kSecMatchSearchList as String] = [keychain] }
        return query
    }

    func access(_ protection: Protection, label: String) -> SecAccess? {
        var trusted: [SecTrustedApplication] = []
        if protection == .pinnedHelper {
            var own: SecTrustedApplication?
            guard SecTrustedApplicationCreateFromPath(nil, &own) == errSecSuccess, let own else { return nil }
            trusted = [own]
        }
        var access: SecAccess?
        guard SecAccessCreate(label as CFString, trusted as CFArray, &access) == errSecSuccess else { return nil }
        return access
    }

    /// Creates a new item only. An existing item is never overwritten.
    public func put(reference: String, secret: Data) -> Status {
        let label = "Mailroom: \(reference)"
        guard validReference(reference), !secret.isEmpty,
              let access = access(Protection(reference: reference), label: label) else { return .failed }
        var add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: reference,
            kSecAttrLabel as String: label,
            kSecValueData as String: secret,
            kSecAttrAccess as String: access,
        ]
        if let keychain { add[kSecUseKeychain as String] = keychain }
        return Self.map(SecItemAdd(add as CFDictionary, nil))
    }

    public func get(reference: String) -> (Status, Data?) {
        read(Self.service, reference)
    }

    public func legacyGet(reference: String) -> (Status, Data?) {
        read(Self.legacyService, reference)
    }

    func read(_ service: String, _ reference: String) -> (Status, Data?) {
        guard validReference(reference) else { return (.failed, nil) }
        var query = query(service, reference)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = Self.map(SecItemCopyMatching(query as CFDictionary, &result))
        guard status == .ok, let data = result as? Data, !data.isEmpty else { return (status == .ok ? .failed : status, nil) }
        return (.ok, data)
    }

    public func delete(reference: String) -> Status {
        remove(Self.service, reference)
    }

    public func legacyDelete(reference: String) -> Status {
        remove(Self.legacyService, reference)
    }

    func remove(_ service: String, _ reference: String) -> Status {
        guard validReference(reference) else { return .failed }
        guard let item = item(service, reference) else { return exists(service, reference) }
        return Self.map(SecKeychainItemDelete(item))
    }

    func item(_ service: String, _ reference: String) -> SecKeychainItem? {
        var query = query(service, reference)
        query[kSecReturnRef as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let result,
              CFGetTypeID(result) == SecKeychainItemGetTypeID() else { return nil }
        return (result as! SecKeychainItem)
    }

    func exists(_ service: String, _ reference: String) -> Status {
        var query = query(service, reference)
        query[kSecReturnAttributes as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = Self.map(SecItemCopyMatching(query as CFDictionary, &result))
        return status == .ok ? .failed : status
    }

    /// Reads the item's access list, never its data.
    public func protection(reference: String) -> (Status, Protection?) {
        guard validReference(reference) else { return (.failed, nil) }
        guard let item = item(Self.service, reference) else { return (exists(Self.service, reference), nil) }
        var access: SecAccess?
        guard SecKeychainItemCopyAccess(item, &access) == errSecSuccess, let access,
              let lists = SecAccessCopyMatchingACLList(access, kSecACLAuthorizationDecrypt) as? [SecACL],
              !lists.isEmpty else { return (.failed, nil) }
        var counts = Set<Int>()
        for acl in lists {
            var applications: CFArray?
            var description: CFString?
            var selector = SecKeychainPromptSelector()
            guard SecACLCopyContents(acl, &applications, &description, &selector) == errSecSuccess,
                  let applications = applications as? [SecTrustedApplication] else { return (.ok, nil) }
            counts.insert(applications.count)
        }
        switch counts {
        case [0]: return (.ok, .everyAccess)
        case [1]: return (.ok, .pinnedHelper)
        default: return (.ok, nil)
        }
    }

    /// Lists legacy `keyring` item names from their attributes. Values are never read here.
    public func legacyReferences() -> (Status, [String]) {
        var query = query(Self.legacyService, nil)
        query[kSecReturnAttributes as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = Self.map(SecItemCopyMatching(query as CFDictionary, &result))
        if status == .missing { return (.ok, []) }
        guard status == .ok, let rows = result as? [[String: Any]] else { return (status == .ok ? .failed : status, []) }
        let references = rows.compactMap { $0[kSecAttrAccount as String] as? String }.filter(validReference).sorted()
        return (.ok, references)
    }
}
