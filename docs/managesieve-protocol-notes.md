# ManageSieve read protocol notes

[RFC 5804 section 1.6](https://www.rfc-editor.org/rfc/rfc5804.html#section-1.6) requires nonempty UTF-8 script names conforming to Net-Unicode. It excludes C0/C1 controls, DEL, U+2028 and U+2029. Servers must support 128 Unicode characters and may permit longer names. Mailroom applies an explicit 1024-byte local name bound and never truncates a name.

[RFC 5198 section 2](https://www.rfc-editor.org/rfc/rfc5198.html#section-2) prohibits an initial BOM and transmitting unassigned code points. NFC normalization is a SHOULD. Discovery preserves returned names when issuing GETSCRIPT, because changing a resource identifier can select a different script. It preserves script contents byte for byte. The already-locked ICU 2.4 general-category table identifies unassigned code points. A direct dependency exposes that existing table; no package versions or TLS dependencies change.

[RFC 5804 section 4](https://www.rfc-editor.org/rfc/rfc5804.html#section-4) counts literal octets, limits quoted strings to 1024 octets between quotes, requires valid UTF-8 inside quoted strings and restricts escapes to quote/backslash. Literal strings contain counted arbitrary octets. Wire escape bytes count toward the quoted-string limit. Unsigned 32-bit integers have no extra leading zeroes. Unknown response codes must be tolerated within local token/resource bounds; response-code names cannot begin with a slash.

[RFC 5804 section 2.2](https://www.rfc-editor.org/rfc/rfc5804.html#section-2.2) requires consuming the new capability response after STARTTLS and discarding earlier capabilities. The new response must omit STARTTLS. [Section 2.1](https://www.rfc-editor.org/rfc/rfc5804.html#section-2.1) and the authentication response grammar require omitting unsolicited post-authentication capabilities when no SASL security layer was negotiated. PLAIN establishes no such layer, so the client explicitly issues CAPABILITY after the authentication OK.

These are protocol requirements and implementation limits. They establish no provider endpoint, account fact, coexistence decision or deployment authority.
