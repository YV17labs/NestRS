//! [`RedisTls`] — what a `rediss://` connection trusts, and what it presents.
//!
//! The URL's scheme decides whether a connection is encrypted, as it does for
//! every Redis client: `rediss://` is TLS, `redis://` is plaintext. A TLS
//! connection verifies Redis's certificate for the URL's host against the
//! authorities of Mozilla's root program, compiled into the client
//! (`webpki-roots`). The system's own store is never read: `redis` would read
//! it again for every connection it opens — every reconnection's included —
//! blocking the runtime while it does, and fail the connection whenever the
//! store could not be read, a configured authority notwithstanding. Naming the
//! system's bundle in `<PREFIX>_REDIS__TLS_CA_CERT_FILE` trusts its authorities
//! instead, read once.
//!
//! [`RedisTls`] changes what is trusted and what is presented — a private
//! authority, a client certificate for a Redis that requires one — and is
//! refused beside a plaintext URL, since whoever configured it expects it to be
//! used.
//!
//! **Verification is never switched off.** `rediss://…#insecure` is refused at
//! boot, and the client is built without the code that would honour it
//! (`redis`'s `tls-rustls-insecure`): a certificate a private authority signed
//! is trusted by configuring that authority, which keeps the connection
//! authenticated as well as encrypted.

use std::fmt;
use std::io;
use std::sync::Arc;

use nest_rs_config::{ConfigError, ConfigService, Material, Result, Setting};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;

/// TLS material for a `rediss://` connection. The default — nothing set —
/// trusts Mozilla's root program and presents no certificate.
///
/// Read once, when the configuration resolves: a connection reopened later
/// reuses it, and a renewed file takes effect at the next boot.
#[derive(Clone, Default)]
pub struct RedisTls {
    /// PEM certificates of the authorities Redis's certificate must chain to,
    /// **replacing** the compiled-in ones — for a Redis a private authority
    /// signed, or the system's bundle, to trust the system's authorities. Read
    /// from `<PREFIX>_REDIS__TLS_CA_CERT`, or the file
    /// `<PREFIX>_REDIS__TLS_CA_CERT_FILE` names.
    pub ca_cert: Option<Vec<u8>>,
    /// The certificate this process presents, for a Redis that requires one —
    /// its default once TLS is on (`tls-auth-clients yes`). Read from
    /// `<PREFIX>_REDIS__TLS_CERT` and `<PREFIX>_REDIS__TLS_KEY`, or their `_FILE`
    /// forms, both or neither.
    pub identity: Option<RedisTlsIdentity>,
}

/// A client certificate and the private key it certifies, both PEM.
#[derive(Clone)]
pub struct RedisTlsIdentity {
    /// The certificate chain, leaf first.
    pub cert: Vec<u8>,
    /// The leaf's private key.
    pub key: Vec<u8>,
}

impl fmt::Debug for RedisTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisTls")
            .field("ca_cert", &self.ca_cert.as_ref().map(|_| "<pem>"))
            .field("identity", &self.identity)
            .finish()
    }
}

impl fmt::Debug for RedisTlsIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisTlsIdentity")
            .field("cert", &"<pem>")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl RedisTls {
    /// Overlay the deployment's TLS variables on `base`, per field — the
    /// certificate and its key as one field, since half a pair can never be
    /// presented.
    pub(crate) fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let ca_cert = match env.material("TLS_CA_CERT")? {
            Some(pem) => Some(pem.value.bytes),
            None => base.ca_cert,
        };
        let identity = match (env.material("TLS_CERT")?, env.material("TLS_KEY")?) {
            (Some(cert), Some(key)) => Some(RedisTlsIdentity {
                cert: cert.value.bytes,
                key: key.value.bytes,
            }),
            (None, None) => base.identity,
            (Some(cert), None) => {
                return Err(half_an_identity(env, &cert, "TLS_KEY"));
            }
            (None, Some(key)) => {
                return Err(half_an_identity(env, &key, "TLS_CERT"));
            }
        };
        Ok(Self { ca_cert, identity })
    }

    /// Whether anything is configured — which only a `rediss://` URL can use.
    pub(crate) fn is_set(&self) -> bool {
        self.ca_cert.is_some() || self.identity.is_some()
    }

    /// Refuse material no handshake could use, before anything is dialled, with
    /// a reason naming the variables that hold it. The client would meet it on
    /// every connection it opens instead: an authority file holding no
    /// certificate trusts nothing, so every certificate fails as untrusted, and
    /// a certificate its key does not sign for is refused by rustls as a client
    /// configuration error that names no variable.
    ///
    /// The pair is judged by rustls's own `CertifiedKey::from_der` — the check
    /// the client runs — so a key whose provider cannot say which public key it
    /// holds passes here exactly as it passes there. A parse failure is
    /// described and never quoted: the parser's error carries the line it choked
    /// on, byte for byte, and a key whose line breaks were collapsed is one line
    /// holding all of it.
    pub(crate) fn check(&self, provider: &CryptoProvider) -> std::result::Result<(), String> {
        if let Some(ca_cert) = &self.ca_cert {
            let authorities =
                certificates(ca_cert).ok_or_else(|| not_pem(&spellings("TLS_CA_CERT")))?;
            if authorities.is_empty() {
                return Err(format!(
                    "{} holds no CERTIFICATE block, so it would trust no certificate",
                    spellings("TLS_CA_CERT")
                ));
            }
        }
        if let Some(identity) = &self.identity {
            let chain =
                certificates(&identity.cert).ok_or_else(|| not_pem(&spellings("TLS_CERT")))?;
            if chain.is_empty() {
                return Err(format!(
                    "{} holds no CERTIFICATE block",
                    spellings("TLS_CERT")
                ));
            }
            let key =
                PrivateKeyDer::from_pem_slice(&identity.key).map_err(|error| match error {
                    rustls::pki_types::pem::Error::NoItemsFound => format!(
                        "{} holds no PRIVATE KEY block — an empty value, a certificate or a \
                         public key in its place, or an encrypted key, which has to be \
                         decrypted first",
                        spellings("TLS_KEY")
                    ),
                    _ => not_pem(&spellings("TLS_KEY")),
                })?;
            provider
                .key_provider
                .load_private_key(key.clone_key())
                .map_err(|error| {
                    format!(
                        "{} is not a key rustls can sign with: {error}",
                        spellings("TLS_KEY")
                    )
                })?;
            CertifiedKey::from_der(chain, key, provider).map_err(|error| {
                format!(
                    "{} is not the certificate of the key in {}: {error}",
                    spellings("TLS_CERT"),
                    spellings("TLS_KEY")
                )
            })?;
        }
        Ok(())
    }

    /// The material in the shape the client takes it.
    pub(crate) fn certificates(&self) -> redis::TlsCertificates {
        redis::TlsCertificates {
            client_tls: self
                .identity
                .as_ref()
                .map(|identity| redis::ClientTlsConfig {
                    client_cert: identity.cert.clone(),
                    client_key: identity.key.clone(),
                }),
            root_cert: self.ca_cert.clone(),
        }
    }
}

/// Why the client refused material [`RedisTls::check`] let through. Both parse
/// the same PEM with the same parser, so what is left is an authority whose
/// certificate rustls cannot make a trust anchor of.
pub(crate) fn unusable_material() -> String {
    format!(
        "{} holds a certificate rustls cannot use as an authority",
        spellings("TLS_CA_CERT")
    )
}

/// The process-wide crypto provider rustls builds with, installed first when
/// the app has not chosen one.
///
/// `redis` builds its client configuration from that default, and rustls picks
/// one on its own only when exactly one of its providers is compiled in: a
/// dependency tree enabling both — this workspace's does — leaves it nothing to
/// pick, and the first handshake panics. aws-lc-rs is rustls's own default, and
/// the one poem's TLS listener installs under the same condition, so whichever
/// of the two runs first, the process ends with one provider and both use it.
/// An app that installed a provider before the boot keeps its choice.
pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    if let Some(installed) = CryptoProvider::get_default() {
        return Arc::clone(installed);
    }
    // Losing a race to another installer leaves theirs in place, and theirs is
    // the one handed back.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "losing the race leaves the other installer's provider, which is read back below"
    )]
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    CryptoProvider::get_default().map_or_else(
        || Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        Arc::clone,
    )
}

/// Whether `error` is a TLS negotiation every attempt would fail the same way —
/// a certificate the client does not accept, a handshake Redis refused, a peer
/// answering in something other than TLS. None of it is an outage, so the boot
/// fails at once. An internal error Redis reports may clear, and is retried like
/// an outage; so is a peer that never answers the handshake, which reaches the
/// boot only as the timeout it cannot be told apart from; and so is a failure of
/// this host's own TLS stack — no clock, no random bytes, a configuration rustls
/// rejects — which no setting of Redis's or the peer's fixes.
pub(crate) fn negotiation_failed(error: &(dyn std::error::Error + 'static)) -> bool {
    negotiation_error(error).is_some_and(|refused| {
        !matches!(
            refused,
            rustls::Error::AlertReceived(rustls::AlertDescription::InternalError)
                | rustls::Error::General(_)
                | rustls::Error::FailedToGetCurrentTime
                | rustls::Error::FailedToGetRandomBytes
        )
    })
}

/// What to change about a failed negotiation, naming the setting. rustls's own
/// reason is not repeated here: the error carrying this sentence keeps it as
/// its source.
pub(crate) fn remedy(error: &(dyn std::error::Error + 'static)) -> String {
    use rustls::{AlertDescription as Alert, CertificateError as Certificate, Error as Tls};

    let Some(refused) = negotiation_error(error) else {
        return format!("check {} and the TLS settings beside it", spellings("URL"));
    };
    match refused {
        Tls::InvalidCertificate(
            Certificate::Expired
            | Certificate::ExpiredContext { .. }
            | Certificate::NotValidYet
            | Certificate::NotValidYetContext { .. },
        ) => "Redis's certificate is outside its validity period — renew it, or check this \
              host's clock"
            .to_owned(),
        Tls::InvalidCertificate(
            Certificate::NotValidForName | Certificate::NotValidForNameContext { .. },
        ) => format!(
            "Redis's certificate names no host matching the one {} dials — a certificate names \
             its hosts in subjectAltName, and its common name is not read: dial a name it \
             carries, or reissue it",
            var("URL")
        ),
        Tls::InvalidCertificate(Certificate::UnknownIssuer | Certificate::BadSignature) => {
            format!(
                "Redis's certificate does not chain to an authority this app trusts — set {} to \
                 the authority that signed it, or have Redis serve the intermediate certificates \
                 that lead to one",
                spellings("TLS_CA_CERT")
            )
        }
        Tls::InvalidCertificate(
            Certificate::InvalidPurpose | Certificate::InvalidPurposeContext { .. },
        ) => "Redis's certificate is not issued for server authentication — reissue it with the \
              serverAuth extended key usage"
            .to_owned(),
        Tls::InvalidCertificate(_) => {
            "Redis's certificate cannot serve as a server certificate — reissue it".to_owned()
        }
        Tls::AlertReceived(Alert::ProtocolVersion) | Tls::PeerIncompatible(_) => {
            "Redis's TLS settings share nothing with this client, which speaks TLS 1.2 and 1.3 \
             with rustls's default cipher suites — check Redis's tls-protocols, tls-ciphers \
             (TLS 1.2) and tls-ciphersuites (TLS 1.3)"
                .to_owned()
        }
        Tls::AlertReceived(Alert::HandshakeFailure) => format!(
            "Redis refused the handshake — it requires a client certificate, set through {} and \
             {}, or shares no cipher suite, key-exchange group or certificate key type with this \
             client (Redis's tls-ciphers for TLS 1.2, tls-ciphersuites for TLS 1.3, and the key \
             its certificate holds)",
            spellings("TLS_CERT"),
            spellings("TLS_KEY")
        ),
        Tls::AlertReceived(_) => format!(
            "Redis refused the handshake — if it requires a client certificate, set {} and {} to \
             one it accepts",
            spellings("TLS_CERT"),
            spellings("TLS_KEY")
        ),
        _ => format!(
            "the peer did not complete a TLS handshake — check that the port {} dials serves TLS",
            var("URL")
        ),
    }
}

/// The rustls error a failed negotiation carries, when `error` is one.
fn negotiation_error<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a rustls::Error> {
    let mut next = Some(error);
    while let Some(current) = next {
        // `redis` keeps the error it wraps behind an `Arc`, whose own `source`
        // skips it: look through the `Arc` before anything else.
        let current = current
            .downcast_ref::<std::sync::Arc<dyn std::error::Error + Send + Sync>>()
            .map_or(current, |shared| {
                &**shared as &(dyn std::error::Error + 'static)
            });
        if let Some(refused) = current.downcast_ref::<rustls::Error>() {
            return Some(refused);
        }
        // An io error's `source` skips the error it wraps, which is where
        // rustls's travels: tokio-rustls hands a failed handshake back as an
        // `InvalidData` io error around it, and `redis` keeps that io error.
        if let Some(refused) = current
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        {
            return Some(refused);
        }
        next = current.source();
    }
    None
}

fn certificates(pem: &[u8]) -> Option<Vec<CertificateDer<'static>>> {
    CertificateDer::pem_slice_iter(pem)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()
}

fn not_pem(what: &str) -> String {
    format!(
        "{what} is not valid PEM: each -----BEGIN and -----END line stands on a line of its own, \
         with intact base64 between them, and a value whose line breaks were collapsed or \
         escaped does not parse"
    )
}

/// Both variables a PEM setting is read from, as the operator sets one of them.
fn spellings(key: &str) -> String {
    nest_rs_config::spellings("redis", key)
}

fn var(key: &str) -> String {
    nest_rs_config::var_name("redis", key)
}

fn half_an_identity(env: &ConfigService, given: &Setting<Material>, missing: &str) -> ConfigError {
    given.refuse(format_args!(
        "is set without {} — a client certificate is presented with its key",
        env.spellings(missing),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT_CERT: &[u8] = include_bytes!("../tests/harness/fixtures/tls_client.pem");
    const CLIENT_KEY: &[u8] = include_bytes!("../tests/harness/fixtures/tls_client.key.pem");

    #[test]
    fn the_deployment_overlays_each_field_and_the_certificate_moves_with_its_key() {
        let pinned = RedisTls {
            ca_cert: Some(b"pinned-authority".to_vec()),
            identity: Some(RedisTlsIdentity {
                cert: b"pinned-cert".to_vec(),
                key: b"pinned-key".to_vec(),
            }),
        };
        let tls = RedisTls::from_env(
            &ConfigService::with_vars("redis", [("TLS_CA_CERT", "deployed-authority")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(
            tls.ca_cert.as_deref(),
            Some(&b"deployed-authority"[..]),
            "the deployment outranks the pin"
        );
        let identity = tls.identity.expect("the untouched pair survives");
        assert_eq!(identity.cert, b"pinned-cert");
        assert_eq!(identity.key, b"pinned-key");

        let replaced = RedisTls::from_env(
            &ConfigService::with_vars(
                "redis",
                [("TLS_CERT", "deployed-cert"), ("TLS_KEY", "deployed-key")],
            ),
            RedisTls::default(),
        )
        .expect("a whole pair resolves")
        .identity
        .expect("and is set");
        assert_eq!(replaced.cert, b"deployed-cert");
        assert_eq!(replaced.key, b"deployed-key");
    }

    /// Half a pair is refused naming the spelling the operator set — a file's
    /// variable when a file was named — and both spellings of the half missing.
    #[test]
    fn half_a_client_certificate_is_refused_naming_what_was_set_and_what_is_missing() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/harness/fixtures/tls_client.pem"
        );
        for (key, value, named, missing) in [
            ("TLS_CERT", "-----PEM-----", "TLS_CERT", "TLS_KEY"),
            ("TLS_KEY", "-----PEM-----", "TLS_KEY", "TLS_CERT"),
            ("TLS_CERT_FILE", fixture, "TLS_CERT_FILE", "TLS_KEY"),
        ] {
            let err = RedisTls::from_env(
                &ConfigService::with_vars("redis", [(key, value)]),
                RedisTls::default(),
            )
            .expect_err("half a pair is refused");
            let rendered = err.to_string();
            assert!(
                rendered.contains(&format!("{}:", var(named))),
                "names what was set: {rendered}"
            );
            assert!(
                rendered.contains(&var(missing))
                    && rendered.contains(&var(&format!("{missing}_FILE"))),
                "and both spellings of what is missing: {rendered}"
            );
        }
    }

    /// A key whose line breaks were collapsed is one line holding the whole key,
    /// and the parser's error carries that line byte for byte — so the refusal
    /// describes the problem and quotes nothing, not even as a list of bytes.
    #[test]
    fn material_that_does_not_parse_is_described_and_never_quoted() {
        let text = std::str::from_utf8(CLIENT_KEY).expect("PEM is text");
        let secret = text.lines().nth(1).expect("a base64 line of the key");
        let as_bytes = secret.as_bytes()[..4]
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for collapsed in [text.replace('\n', " "), text.replace('\n', "\\n")] {
            let tls = RedisTls {
                ca_cert: None,
                identity: Some(RedisTlsIdentity {
                    cert: CLIENT_CERT.to_vec(),
                    key: collapsed.into_bytes(),
                }),
            };
            let reason = tls
                .check(&rustls::crypto::aws_lc_rs::default_provider())
                .expect_err("a key on one line does not parse");
            assert!(
                reason.contains(&var("TLS_KEY")),
                "names the setting: {reason}"
            );
            assert!(
                !reason.contains(&secret[..12]),
                "never quotes the key: {reason}"
            );
            assert!(!reason.contains(&as_bytes), "not even as bytes: {reason}");
        }
    }

    /// A pair whose provider cannot say which public key it holds is accepted,
    /// as rustls itself accepts it when the client is built: the check defers to
    /// rustls's verdict rather than a stricter copy of it.
    #[test]
    fn a_key_whose_provider_hides_its_public_half_is_accepted_as_rustls_accepts_it() {
        #[derive(Debug)]
        struct Opaque(Arc<dyn rustls::sign::SigningKey>);
        impl rustls::sign::SigningKey for Opaque {
            fn choose_scheme(
                &self,
                offered: &[rustls::SignatureScheme],
            ) -> Option<Box<dyn rustls::sign::Signer>> {
                self.0.choose_scheme(offered)
            }
            fn algorithm(&self) -> rustls::SignatureAlgorithm {
                self.0.algorithm()
            }
        }
        #[derive(Debug)]
        struct OpaqueKeys;
        impl rustls::crypto::KeyProvider for OpaqueKeys {
            fn load_private_key(
                &self,
                key: PrivateKeyDer<'static>,
            ) -> std::result::Result<Arc<dyn rustls::sign::SigningKey>, rustls::Error> {
                let loaded = rustls::crypto::aws_lc_rs::default_provider()
                    .key_provider
                    .load_private_key(key)?;
                Ok(Arc::new(Opaque(loaded)))
            }
        }
        let provider = CryptoProvider {
            key_provider: &OpaqueKeys,
            ..rustls::crypto::aws_lc_rs::default_provider()
        };
        let tls = RedisTls {
            ca_cert: None,
            identity: Some(RedisTlsIdentity {
                cert: CLIENT_CERT.to_vec(),
                key: CLIENT_KEY.to_vec(),
            }),
        };
        assert_eq!(tls.check(&provider), Ok(()));
    }

    /// `redis` builds every TLS client from the process default, which rustls
    /// never installs on its own in a tree compiling both of its providers.
    #[test]
    fn opening_a_tls_client_leaves_a_process_default_provider_installed() {
        let provider = crypto_provider();
        let installed = CryptoProvider::get_default()
            .expect("a provider is installed for `redis` to build with");
        assert!(
            Arc::ptr_eq(&provider, installed),
            "and it is the one handed back"
        );
    }

    fn negotiation(refused: rustls::Error) -> redis::RedisError {
        redis::RedisError::from(io::Error::new(io::ErrorKind::InvalidData, refused))
    }

    /// A refused handshake fails the boot at once, while a dropped connection
    /// and an internal error on Redis's side are retried — so none of them may
    /// read like another.
    #[test]
    fn a_refused_handshake_is_told_apart_from_what_may_clear() {
        let refused = negotiation(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        ));
        let internal = negotiation(rustls::Error::AlertReceived(
            rustls::AlertDescription::InternalError,
        ));
        let dropped = redis::RedisError::from(io::Error::from(io::ErrorKind::ConnectionReset));
        let local = negotiation(rustls::Error::FailedToGetCurrentTime);
        let rejected_here = negotiation(rustls::Error::General("no usable configuration".into()));
        assert!(negotiation_failed(&refused), "{refused}");
        assert!(!negotiation_failed(&internal), "{internal}");
        assert!(!negotiation_failed(&dropped), "{dropped}");
        assert!(!negotiation_failed(&local), "{local}");
        assert!(!negotiation_failed(&rejected_here), "{rejected_here}");
    }

    /// Each way a handshake fails sends the operator to the setting that fixes
    /// it — the authority, the host, the clock, Redis's own certificate, the
    /// protocol settings, the client certificate, the port — and says the cause
    /// that setting fixes, so no two causes share a sentence. rustls's reason
    /// is not repeated: the error keeps it as its source.
    #[test]
    fn a_refusal_names_the_setting_that_fixes_its_cause() {
        use rustls::{AlertDescription, CertificateError, Error, InvalidMessage, PeerIncompatible};

        for (refused, says, not) in [
            (
                Error::InvalidCertificate(CertificateError::UnknownIssuer),
                format!("set {}", spellings("TLS_CA_CERT")),
                var("URL"),
            ),
            (
                Error::InvalidCertificate(CertificateError::BadSignature),
                format!("set {}", spellings("TLS_CA_CERT")),
                var("URL"),
            ),
            (
                Error::InvalidCertificate(CertificateError::NotValidForName),
                "subjectAltName".to_owned(),
                var("TLS_CA_CERT"),
            ),
            (
                Error::InvalidCertificate(CertificateError::Expired),
                "clock".to_owned(),
                var("TLS_CA_CERT"),
            ),
            (
                Error::InvalidCertificate(CertificateError::InvalidPurpose),
                "serverAuth".to_owned(),
                var("TLS_CA_CERT"),
            ),
            (
                Error::InvalidCertificate(CertificateError::BadEncoding),
                "cannot serve as a server certificate".to_owned(),
                var("TLS_CA_CERT"),
            ),
            (
                Error::AlertReceived(AlertDescription::ProtocolVersion),
                "tls-protocols".to_owned(),
                var("TLS_KEY"),
            ),
            (
                Error::PeerIncompatible(PeerIncompatible::NoCipherSuitesInCommon),
                "tls-protocols".to_owned(),
                var("TLS_KEY"),
            ),
            (
                Error::AlertReceived(AlertDescription::HandshakeFailure),
                "tls-ciphers for TLS 1.2".to_owned(),
                var("TLS_CA_CERT"),
            ),
            (
                Error::AlertReceived(AlertDescription::CertificateRequired),
                format!("set {}", spellings("TLS_CERT")),
                "tls-ciphersuites".to_owned(),
            ),
            (
                Error::InvalidMessage(InvalidMessage::InvalidContentType),
                "serves TLS".to_owned(),
                "subjectAltName".to_owned(),
            ),
        ] {
            let reason = refused.to_string();
            let sentence = remedy(&negotiation(refused));
            assert!(sentence.contains(&says), "says {says}: {sentence}");
            assert!(!sentence.contains(&not), "and not {not}: {sentence}");
            assert!(
                !sentence.contains(&reason),
                "and leaves rustls's reason to the source: {sentence}"
            );
        }
    }

    /// A key setting holding no private key — empty, or a certificate in its
    /// place — says so, rather than blaming line breaks it does not have.
    #[test]
    fn a_key_setting_holding_no_private_key_says_so() {
        for (label, key) in [
            ("empty", Vec::new()),
            ("a certificate", CLIENT_CERT.to_vec()),
        ] {
            let tls = RedisTls {
                ca_cert: None,
                identity: Some(RedisTlsIdentity {
                    cert: CLIENT_CERT.to_vec(),
                    key,
                }),
            };
            let reason = tls
                .check(&rustls::crypto::aws_lc_rs::default_provider())
                .expect_err("a setting with no key in it is refused");
            assert!(reason.contains("no PRIVATE KEY block"), "{label}: {reason}");
        }
    }

    #[test]
    fn debug_never_shows_the_private_key() {
        let tls = RedisTls {
            ca_cert: None,
            identity: Some(RedisTlsIdentity {
                cert: b"cert".to_vec(),
                key: b"s3cr3t".to_vec(),
            }),
        };
        let shown = format!("{tls:?}");
        assert!(!shown.contains("s3cr3t"), "{shown}");
    }
}
