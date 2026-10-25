# Exact-action confirmation

The invoice and subscription review routes use expiring server-owned requests for synthetic confirmation. The UI displays the action derived from the request. Each request names the account, purpose, revision, target, applicable sending and receiving identities, source and content digests. The full existing generation-bound HMAC receipt remains part of the internal binding.

The public `receipt` is an opaque random request identity. It is valid for at most five minutes and one attempted confirmation. Requests disappear on process replacement. Refresh the preview after a restart or a rejected confirmation. A browser checkbox or action string alone grants no authority.

The shared boundary verifies an explicitly installed helper identity, location and owner before requesting consent. It rejects substitution, cancellation, failed or malformed outcomes and unauthenticated responses. It reloads the encrypted state, checks the current ledger source and recomputes the exact plan after consent. Relevant application and store locks remain held through admission. Noncooperating filesystem edits remain outside the existing advisory-lock guarantee.

The boundary returns an internal consumed-consent capability. The route still needs its original purpose-specific receipt and durable journal admission. Intent is saved before a simulated effect, and observation is saved afterward. Process replacement leaves interrupted effects uncertain until reconciliation. Neither native consent nor a fresh preview permits automatic retry. Sends and external unsubscribes have no mailbox-style Undo.

## Native status

Native execution is unavailable. The Rust native backend denies without starting a process. The Swift source compiles, but its trusted application and helper requirements are absent. Static signature checking followed by an ordinary path-based spawn does not prevent replacement between checking and execution. A protected executable and authenticated launch channel must be established before any native invocation can be enabled.

The helper source uses LocalAuthentication with a structured action-naming reason and a fresh context. It checks expected running identities and returns an authenticated request-bound outcome through pipes. Compilation does not prove actual prompt visibility, signing, installation, application trust, private-channel integrity or Keychain behavior. See [Apple API findings](macos-confirmation-research.md) for evidence and limits.

The macOS production credential adapter also denies before keyring access. Synthetic credential replacement, creation, forgetting and rotation continue through the existing injected lifecycle backend. No encrypted stores, selected generations, stable identity references or journals were migrated by this change.

Invoice mail-app forwarding and unsubscribe browser or mail-app copy cards remain preparation only. Nothing launches or sends through them. The interface labels all fake outcomes as synthetic. Provider writes stay independently denied.

## Compile and inspect the owner setup plan

Run these commands from the checkout:

```sh
mkdir -p .audit
swiftc -parse-as-library native/macos/ConfirmationHelper.swift -o .audit/macos-confirmation-helper
bash -n scripts/setup-macos-protection.sh
bash scripts/setup-macos-protection.sh --plan
```

Do not invoke the compiled helper for synthetic verification. The owner-run wizard is a prerequisite walkthrough. Its planning mode has no side effects. The interactive path writes no configuration and cannot install helpers, enter credentials or enable invocation. It ends with an explicit incomplete-acceptance status.

The owner must choose exact trusted signing requirements. Future implementation and acceptance must establish protected launch, real prompt cancellation and replay behavior, application-specific Keychain trust and approval on every access before private credential entry. The later encrypted-recovery feature needs a bundle outside the Mac and a separately held secret. This walkthrough creates neither and chooses no storage provider, schedule, retention or escrow policy.
