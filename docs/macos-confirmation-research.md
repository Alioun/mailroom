# macOS confirmation API findings

These findings were checked against Apple documentation and Apple Security source on 2026-10-25. They describe API contracts. No native authentication, Keychain access, signing, installation or provisioning was performed. Retrieved source snapshots remain in `.audit/macos-confirmation-research/`.

## Action-naming authentication

[`LAContext.evaluatePolicy`](https://developer.apple.com/documentation/localauthentication/lacontext/evaluatepolicy(_:localizedreason:reply:)) accepts an app-provided reason that appears in the authentication dialog. Apple asks the app to describe the resulting action clearly and in the user's language. Authentication can fail or be cancelled. A previous success does not establish a future success.

The helper derives the reason from structured server action fields. It uses a fresh context with Touch ID reuse duration zero, then invalidates that context. Apple's [reuse-duration documentation](https://developer.apple.com/documentation/localauthentication/lacontext/touchidauthenticationallowablereuseduration) says zero disables reuse of a recent device-unlock authentication. This does not prove that the complete reason is visible on this Mac, nor does it prove per-access Keychain approval. Those require owner-run acceptance. The compiled helper has no provisioned trusted identities and denies before authentication.

## Expected signing identity

[`SecRequirementCreateWithString`](https://developer.apple.com/documentation/security/secrequirementcreatewithstring(_:_:_:)) compiles a code requirement. [`SecStaticCodeCheckValidityWithErrors`](https://developer.apple.com/documentation/security/secstaticcodecheckvaliditywitherrors(_:_:_:_:)) validates signed code and applies an additional requirement when supplied. Accepting a valid signature with no expected requirement is insufficient.

Apple states, "This call is only secure if the code is not subject to concurrent modification, and the outcome is only valid as long as the code remains unmodified." It also describes the architecture-selection limits of universal-binary verification. The helper's compile-only verifier passes an explicit requirement and the all-architectures and strict-validation flags. The flag definitions are in Apple's [SecStaticCode.h](https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_codesigning/lib/SecStaticCode.h).

A checked path followed by `Command::spawn`, `Process.run` or a shell launch would not satisfy this contract. No substitution-resistant Rust launch implementation was established in this unit. Rust therefore has no native spawn path and always denies native invocation. File ownership and path checks alone cannot repair this gap against an uncooperative process running as the same user.

[`SecCodeCopyGuestWithAttributes`](https://developer.apple.com/documentation/security/seccodecopyguestwithattributes(_:_:_:_:)) can identify running code using host-specific attributes. [`SecCodeCheckValidity`](https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_codesigning/lib/SecCode.h) checks dynamic code against a requirement. The Swift source checks its own and its parent's running identities against requirements compiled into the helper. Both requirements are absent in this build. These checks are additional controls. They do not prove pre-invocation executable continuity or authenticate a future launch channel by themselves.

## Credential constraints

Apple's [Keychain accessibility guide](https://developer.apple.com/documentation/security/restricting-keychain-item-accessibility) distinguishes device-unlock accessibility from explicit user presence. [`userPresence`](https://developer.apple.com/documentation/security/secaccesscontrolcreateflags/userpresence) allows an available biometric or device-passcode mechanism. The Swift function constructs a `SecAccessControl` with user presence and device-local unlocked accessibility. It performs no `SecItem` operation.

For macOS, [`kSecUseDataProtectionKeychain`](https://developer.apple.com/documentation/security/ksecusedataprotectionkeychain) selects iOS-style semantics without implying iCloud synchronization. Apple recommends it for new operations. Apple's [access-group guide](https://developer.apple.com/documentation/security/sharing-access-to-keychain-items-among-a-collection-of-apps) describes entitlement-controlled groups and an application's private default group. A future protected adapter needs the owner-selected signed application identity and its correctly provisioned private group. It must verify fresh approval, cancellation and refusal to a different application on the target Mac.

Legacy [`SecAccessCreate`](https://developer.apple.com/documentation/security/secaccesscreate(_:_:_:)) explicitly describes trusted applications as applications allowed restricted access without confirmation dialogs. Its default calling-application trust cannot establish approval on every access. Ordinary `keyring` get, put and delete do not establish the required combination either. This build denies the macOS `OsCredentials` adapter before invoking keyring. Synthetic lifecycle backends remain available. The protected Keychain adapter, entitlement provisioning, migration of legacy entries and per-access owner-run checks remain unimplemented. No acceptance claim is made for them.

## Private outcome channel

The compile-only helper accepts a bounded request and a random per-invocation challenge through inherited pipes, with no secret in argv. It authenticates the request digest, request identity and outcome using HMAC-SHA256. The Rust fake protocol tests reject malformed, oversized, forged and cross-request outcomes. Authentication of this protocol depends on delivering the challenge only to the verified executable. That native delivery guarantee remains open, so the native adapter is denied even if the helper source compiles.

Native consent never grants network-write authority. Real sends, unsubscribe, provider mutations and account credential installation remain independently denied. Encrypted recovery is separate future work. No recovery bundle or real backup was created.
