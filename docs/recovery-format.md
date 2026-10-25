# Recovery bundle profile

This development profile uses JWE Compact Serialization with `alg=dir` and `enc=A256GCM`. The separately held recovery secret is 32 random bytes, not a human password. The protected header is authenticated as its base64url ASCII representation. The IV is 12 bytes and the tag is 16 bytes. Compression and algorithm negotiation are rejected.

Primary sources read before implementation:

- [RFC 7516, sections 5 and 7.1](https://www.rfc-editor.org/rfc/rfc7516#section-5.1) defines encryption, authenticated header bytes and five-part Compact Serialization.
- [RFC 7518, section 4.5](https://www.rfc-editor.org/rfc/rfc7518#section-4.5) defines direct encryption with the shared key and an empty encrypted-key segment.
- [RFC 7518, section 5.3](https://www.rfc-editor.org/rfc/rfc7518#section-5.3) requires a 96-bit IV and a 128-bit tag for AES GCM.
- [RustCrypto aes-gcm 0.10.3](https://docs.rs/aes-gcm/0.10.3/aes_gcm/) supplies authenticated encryption and decryption. Its documented platform restrictions remain applicable.
- [NamedTempFile](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist_noclobber) documents no-clobber publication. File and parent-directory synchronization are explicit. These checks do not establish arbitrary power-loss durability.

A bundle contains one selected account. Its versioned manifest names its identity, state schema from versions 1 through 7, generation, revision, capture boundary, exact ciphertext and metadata inventory, and cross-account report dependencies. The inventory also lists the roles and lengths of required key material without exposing its bytes or digests. Required local encryption material is inside the authenticated ciphertext. Provider passwords are absent. External mailbox and transport stores are excluded. Shared authoritative ledger files are excluded and never overwritten. Captured projections are historical.

Restoration supports absent accounts only and denies collisions. An encrypted transaction copy and a durable account marker precede credential and ciphertext publication. Metadata becomes visible last. Interrupted restoration blocks normal access and requires authenticating the same bundle again. A new authority invalidates approvals while stable HMAC identities and the primary ciphertext remain unchanged. Restored accounts are read-only pending a future authoritative reconciliation workflow. There is no automatic retry, compensation or unlock. Production routes deny before repository, credential or native access.
