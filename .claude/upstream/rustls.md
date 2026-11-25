# rustls

## An error has no sentence free of what its peer wrote

- **Seen** — 0.23.46, `src/error.rs`: `Display for Error` is the one sentence
  an error has, and it quotes. `InvalidCertificate` goes on with what the
  certificate holds — `NotValidForNameContext` its names, `ExpiredContext` and
  `NotValidYetContext` its dates; `General(String)` and `Other(OtherError)`
  print a text whoever raised them wrote; `InvalidMessage`, `PeerMisbehaved`
  and `PeerIncompatible` print their inner variant's `Debug`, and
  `PeerIncompatible::ServerRejectedEncryptedClientHello` holds the
  configurations the server answered (`Option<Vec<EchConfigPayload>>`).
  `Error` and each of its inner enums are `#[non_exhaustive]`, so a 0.23.x may
  add a variant holding a peer's bytes. No method says the failure alone.
- **Effect** — a line saying why a handshake failed either quotes what the peer
  presented, or says no more than the kind of the I/O error holding rustls's
  (`invalid data`), the same for an expired client certificate and for a port
  speaking no TLS.
- **Our side** — `said_tls` in `crates/nest-rs-reqwest/src/safe_link.rs`
  allowlists the outer variants read off 0.23.46 as holding nothing the peer
  wrote but a code of the protocol, and says each by rustls's own `Display`:
  `AlertReceived`, `InvalidMessage`, `InappropriateMessage`,
  `InappropriateHandshakeMessage`, `PeerMisbehaved`, `PeerIncompatible` —
  `ServerRejectedEncryptedClientHello` apart — `NoCertificatesPresented`,
  `DecryptError`, `PeerSentOversizedRecord`, `HandshakeNotComplete`,
  `NoApplicationProtocol`, `UnsupportedNameType`, `EncryptError`,
  `FailedToGetCurrentTime`, `FailedToGetRandomBytes`, `BadMaxFragmentSize`.
  Any other variant, a later one included, is not said by its type;
  `InvalidCertificate` is said by the constant opening of its sentence
  (`KNOWN`, same file). The list is by outer variant: at 0.23.46
  `InvalidMessage`, `PeerMisbehaved` and `PeerIncompatible` hold constants and
  codes alone (`&'static str`, `u16`, `KeyExchangeAlgorithm`), that one
  variant apart. On a rustls bump, `src/error.rs` is read again — each
  variant `Error` gained, and each one `InvalidMessage`, `PeerMisbehaved` or
  `PeerIncompatible` gained that holds data — and the list moved with it.
- **Canary** — none: no test sees a variant a later release adds. The list as
  read is held by `safe_link.rs`'s unit tests.
- **Upstream** — not filed.
- **When fixed** — `below` says what rustls offers for any error; `said_tls`
  and the five `invalid peer certificate` openings of `KNOWN` go. An hour.
- **Ticket** —

  > **A sentence for an `Error` that holds nothing the peer wrote**
  >
  > `Display for Error` is the only way to say why a handshake failed, and for
  > some variants it prints what the peer or its certificate holds: the names
  > and validity dates of `CertificateError`'s `*Context` variants, the text
  > of `General` and `Other`, the `Debug` of an inner enum that may carry
  > data (`PeerIncompatible::ServerRejectedEncryptedClientHello`). A service
  > that must keep peer-supplied data out of its logs has to allowlist
  > variants by hand, and every enum involved is `#[non_exhaustive]`, so the
  > list goes stale silently. Could `Error` offer the failure without its
  > context — a method returning a `&'static str` or a fieldless kind, or an
  > alternate `Display` — documented as never printing data from the peer?
