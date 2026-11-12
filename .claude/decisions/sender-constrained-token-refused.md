# A token bound to a key is refused until the framework can verify the proof

## 2026-11-05 — `cnf` is refused at `verify`

Decided on the owner's mandate of 2026-11-05. `JwtService::verify` ignored `cnf`
(RFC 7800 §3), so a token its issuer bound to a key — DPoP's `jkt` (RFC 9449 §6.1), a
certificate's `x5t#S256` (RFC 8705 §3.1) — passed under `Authorization: Bearer` with no
proof of that key: a thief's copy served as well as its holder's. Seen run: `200`.

**What the standard says, read at the RFC and not recalled.** RFC 9449 §7.2 binds a
resource taking both schemes: "such a protected resource MUST reject a DPoP-bound access
token received as a bearer token". The same section says a resource taking `Bearer` alone
"would most presumably accept" one, and that this "likely simplifies the logistics of
phased upgrades". nestrs serves `Bearer` alone, so the MUST does not bind it, and no page
says it does.

**Taken, by the first value: `verify` refuses a token carrying `cnf`**, whichever member
it names, after the signature and the typing checks, as `AuthError::SenderConstrained`. An
issuer asked for that binding, and a resource reading a bearer token cannot honour it; failing
closed tells the deployment at once, where accepting would drop the binding in silence. A
`cnf` serialized as `null` is absent, as every claim the signer reads is.

- **`SenderConstrained`, not `InvalidToken`.** The client reads `invalid_token` either way
  (RFC 6750 §3.1). The operator's line is the guard's one `warn`, and its `reason`
  `sender_constrained` is what tells a deployment its issuer mints bound tokens. `verify` files
  no line of its own: a second would double-count every denial.
- **Read off the payload again, not through a claims wrapper.** A `#[serde(flatten)]` wrapper
  around `C` buffers every claim and changes what `C` may deserialize from. Decoding the
  payload once more and reading one key measured 340 ns for a 334-byte payload (host, release),
  beside a signature check of tens of microseconds on EdDSA. On HS256, what one variable selects
  and what the JWT page opens with, it is a visible share: 257 to 287 ns beside a decode of 1586
  to 1734 ns, six rounds on a claims struct over the same shape of payload, about 16 % of
  verification. Security comes first, so the refusal stands at that price. `jsonwebtoken`'s
  `dangerous::insecure_decode_claims` does the same work and measured 440 ns beside the first
  figure, a third more on every verified request for five lines saved, so the read stays this
  crate's.
- **Refused: a config key accepting bound tokens as bearers**, the phased upgrade §7.2
  describes. A deployment that wants it asked for no opening a caller holds; it can mint plain
  bearer tokens for this audience. No caller asks.
- **Not covered:** verifying the proof. DPoP is a `Strategy` reading the whole request plus the
  RFC 9728 metadata members; mutual TLS is the peer certificate a `Strategy` does not see yet.
  Each is additive, and this entry is reopened when one is built.
