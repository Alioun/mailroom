# Synthetic IMAP folder provisioning

The create-only transport uses verified implicit TLS and a separately installed, account-bound loopback source. Production and ordinary read installations cannot obtain its private create permit. The transport sends CAPABILITY, one LOGIN, NAMESPACE, complete LIST, exact CREATE and bounded LOGOUT. It performs no DNS lookup or plaintext fallback.

The version-one mapping requires IMAP4rev1, NAMESPACE, one observed empty personal prefix and slash hierarchy. UTF8=ONLY, IMAP4rev2, ambiguous personal namespaces, namespace extensions and incompatible topology reject. Optional UTF8=ACCEPT remains unenabled. Client names use quoted canonical modified UTF-7. Server quoted strings, atoms and fragmented octet-counted literals are bounded. Non-NFC text, combining sequences and Hangul Jamo are explicitly unsupported. No normalization changes an owner's text.

The saved immutable artifact supplies every candidate destination through `CompiledArtifact::folders()`. Required ancestors appear separately and in depth order. Existing selectable leaves require no CREATE. Nonselectable ancestors may be containers, but nonselectable leaves and ancestors forbidding children reject. Wildcards are compared locally against the complete inventory.

The pinned saved-workspace compiler uses unknown recipient authority and omits every filing action. Its current artifact therefore has no folder candidates. The application preserves that behavior and displays an empty plan. No compiler trust, role or recipient-evidence change is included. Actual CREATE journal tests use a separately identified synthetic artifact from the existing sample engine context. They do not establish an end-to-end saved-workspace CREATE workflow.

A purpose-separated receipt binds complete primary state, source, account, credential generation, key and recovery authority, proof, exact plan, wire names and observed topology. Approval is consumed before command intents. Each intent precedes CREATE. Acknowledgment and complete fresh LIST readback remain separate. An interrupted intent is uncertain. Status never resumes work. Reconciliation issues only observations and requires renewed exact review after absence or incomplete work. Matching visible state proves neither attribution nor historical non-occurrence.

The primary journal uses internal schema 12. Old schemas invent no folder journal or fake topology and reject new semantics. Single and selected recovery accept schema 12 and retain read-only holds. The independent fake TLS identity, topology and safe command names live in the existing encrypted mailbox resource. Primary restoration cannot roll back later fake folder effects.

CREATE has no compare-and-swap. A server may recreate a parent removed after the last LIST, or subscribe a new mailbox itself. Complete observations reduce stale plans without excluding that race. The application sends no SUBSCRIBE, DELETE, RENAME, compensation, message mutation or Sieve deployment.

Actual provider namespaces, permissions, quotas, name equivalence, implicit parent and subscription behavior, Sieve mapping and delivery remain unverified. Native confirmation and personal readiness remain outside this unit.
