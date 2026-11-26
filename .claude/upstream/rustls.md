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
  Any other variant, a later one included, is not said by its type. The list
  is by outer variant: at 0.23.46 `InvalidMessage`, `PeerMisbehaved` and
  `PeerIncompatible` hold constants and codes alone (`&'static str`, `u16`,
  `KeyExchangeAlgorithm`), that one variant apart.
  **`InvalidCertificate` is said by `invalid peer certificate` and its constant
  reason**, read off the `CertificateError` it holds (`refused_certificate`,
  same file), never off its sentence: a fieldless variant by rustls's own
  word, its name (`BadEncoding`, `Expired`, `NotValidYet`, `Revoked`,
  `UnhandledCriticalExtension`, `UnknownIssuer`, `UnknownRevocationStatus`,
  `ExpiredRevocationList`, `BadSignature`, `NotValidForName`,
  `InvalidPurpose`, `InvalidOcspResponse`, `ApplicationVerificationFailure`);
  a `*Context` variant by the constant opening of its sentence, without the
  dates, the names or the usages it goes on with (`certificate expired`,
  `certificate not valid yet`, `certificate revocation list expired`,
  `certificate not valid for name`, `certificate does not allow extended key
  usage for server authentication` — the usage this client required, said
  only when it is server or client authentication, never the ones the
  certificate allows); the two holding an algorithm's identifier by the name
  of the variant they replaced (`UnsupportedSignatureAlgorithm`,
  `UnsupportedSignatureAlgorithmForPublicKey`). The deprecated fieldless
  `UnsupportedSignatureAlgorithm`, which webpki 0.103.17 no longer raises, and
  any variant a later release adds are said by the opening alone.
  **`Other` is read by the type it holds**: `rustls::webpki::pki_error` hands
  on, as `Other(OtherError(Arc<webpki::Error>))`, every webpki refusal rustls
  has no variant for — `CaUsedAsEndEntity`, `EndEntityUsedAsCa`,
  `NameConstraintViolation`, `PathLenConstraintViolated`,
  `UnsupportedCertVersion` among them — and `refused_by_webpki` says the
  fieldless variants read off rustls-webpki 0.103.17 by their name, webpki's
  `Display` being its `Debug`. An `Other` a verifier of the app's raised, and
  a webpki variant holding the certificate's names, dates or identifiers, is
  said by the opening alone. `nest-rs-reqwest` names `rustls-webpki` for this,
  at the `major.minor` rustls links: a rustls moving to another webpki minor
  would link two, the downcast would miss, and the reason would fall back to
  the opening alone. On a rustls bump, `src/error.rs` is read again — each
  variant `Error` or `CertificateError` gained, and each one `InvalidMessage`,
  `PeerMisbehaved` or `PeerIncompatible` gained that holds data — with
  `src/webpki/mod.rs`'s `pki_error` and rustls-webpki's `src/error.rs`, the
  workspace's `rustls-webpki` requirement moved to the minor rustls names, and
  the lists moved with them.
- **Canary** — for the types: `nest-rs-reqwest`'s integration suite refuses
  three certificates over real sockets — one an authority re-keyed under its
  name signed (`BadSignature`), an authority's own presented as a server's
  (`CaUsedAsEndEntity`, read through `Other`), one issued for client
  authentication alone — and fails the day rustls's or webpki's error stops
  being the type this crate names. None for a variant a later release adds:
  the lists as read are held by `safe_link.rs`'s unit tests.
- **Upstream** — not filed.
- **When fixed** — `below` says what rustls offers for any error; `said_tls`,
  `refused_certificate`, `refused_by_webpki` and the `rustls-webpki`
  dependency go. An hour.
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
