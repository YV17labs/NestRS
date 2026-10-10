//! [`ClientTls`] — what a TLS client the framework opens trusts and presents,
//! read under its namespace's `TLS_CA_CERT`, `TLS_CERT` and `TLS_KEY`;
//! [`TlsIdentity`], a certificate chain and the key it certifies; and
//! [`crypto_provider`], the process's rustls provider.
//!
//! **Verification is never optional**: no field or method of this vocabulary
//! turns it off, and `clippy.toml` refuses the libraries' own switches.

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::Arc;

use rustls::crypto::{CryptoProvider, KeyProvider};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;

use crate::{ConfigError, ConfigService, Material, Result, Setting, system_authorities};

/// What a TLS client the framework opens trusts and presents: the system's
/// authorities ([`system_authorities`]) unless `TLS_CA_CERT` names others, and a
/// client certificate when `TLS_CERT` and `TLS_KEY` are set. Verification is not
/// optional: no field or method turns it off.
///
/// Read once, when the configuration resolves: a renewed file takes effect at
/// the next boot. Every client the framework opens trusts through
/// [`authorities_pem`](Self::authorities_pem), so the system's store is read
/// one way, once, whatever library dials.
///
/// ```
/// use nest_rs_config::{ClientTls, ConfigService};
///
/// let env = ConfigService::with_vars("redis", []);
/// let tls = ClientTls::from_env(&env, ClientTls::default())?;
/// tls.check(&env)?;
/// assert!(tls.is_empty() && tls.identity_pem().is_none());
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
#[derive(Clone, Default)]
pub struct ClientTls {
    authorities: Option<Material>,
    identity: Option<TlsIdentity>,
}

/// A certificate chain, leaf first, and the private key it certifies, both PEM.
#[derive(Clone)]
pub struct TlsIdentity {
    cert: Material,
    key: Material,
}

impl TlsIdentity {
    /// The chain in `cert` presented with the key in `key`; whether they belong
    /// together is [`ClientTls::check`]'s to judge.
    pub fn new(cert: Material, key: Material) -> Self {
        Self { cert, key }
    }
}

impl fmt::Debug for TlsIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsIdentity")
            .field("cert", &self.cert)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl fmt::Debug for ClientTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientTls")
            .field("authorities", &self.authorities)
            .field("identity", &self.identity)
            .finish()
    }
}

impl ClientTls {
    /// The authorities a peer's certificate must chain to, **replacing** the
    /// system's — `None` trusts the system's — and the identity presented.
    pub fn new(authorities: Option<Material>, identity: Option<TlsIdentity>) -> Self {
        Self {
            authorities,
            identity,
        }
    }

    /// The deployment's `TLS_CA_CERT`, `TLS_CERT` and `TLS_KEY` (each with its
    /// `_FILE` spelling) in `env`'s namespace over `base`, per field. The
    /// certificate and its key move as one: half a pair is refused naming what
    /// was set and both spellings of what is missing.
    pub fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let authorities = match env.material("TLS_CA_CERT")? {
            Some(pem) => Some(pem.value),
            None => base.authorities,
        };
        let identity = match (env.material("TLS_CERT")?, env.material("TLS_KEY")?) {
            (Some(cert), Some(key)) => Some(TlsIdentity::new(cert.value, key.value)),
            (None, None) => base.identity,
            (Some(cert), None) => return Err(half_an_identity(env, &cert, "TLS_KEY")),
            (None, Some(key)) => return Err(half_an_identity(env, &key, "TLS_CERT")),
        };
        Ok(Self {
            authorities,
            identity,
        })
    }

    /// Whether nothing is set: the system's authorities trusted, no certificate
    /// presented.
    pub fn is_empty(&self) -> bool {
        self.authorities.is_none() && self.identity.is_none()
    }

    /// Refuse material no handshake could use — no CERTIFICATE block, an
    /// authority rustls cannot anchor trust in, a key the process's provider
    /// cannot sign with, a certificate that is not the key's — naming `env`'s
    /// variables and never quoting the PEM, whose parser errors carry the line
    /// they choke on.
    pub fn check(&self, env: &ConfigService) -> Result<()> {
        if let Some(authorities) = &self.authorities {
            let refuse =
                |reason: &str| ConfigError::parse(named(env, "TLS_CA_CERT", authorities), reason);
            #[expect(
                clippy::map_err_ignore,
                reason = "the PEM parser's error quotes the line it choked on"
            )]
            let anchors = CertificateDer::pem_slice_iter(&authorities.bytes)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| refuse(NOT_PEM))?;
            if anchors.is_empty() {
                return Err(refuse(
                    "holds no CERTIFICATE block, so it would trust no certificate",
                ));
            }
            let mut store = rustls::RootCertStore::empty();
            for anchor in anchors {
                store.add(anchor).map_err(|error| {
                    refuse(&format!(
                        "holds a certificate rustls cannot use as an authority: {error}"
                    ))
                })?;
            }
        }
        if let Some(identity) = &self.identity {
            let provider = crypto_provider();
            certified_key(
                &identity.cert.bytes,
                &identity.key.bytes,
                provider.key_provider,
            )
            .map_err(|refused| refused.naming(env, identity))?;
        }
        Ok(())
    }

    /// Refuse any material beside a connection `declared_by` declares
    /// plaintext — a variable whose scheme does not encrypt — where it would go
    /// unused and the connection unencrypted.
    pub fn refuse_beside_plaintext(&self, env: &ConfigService, declared_by: &str) -> Result<()> {
        let Some((key, material)) = self.first_set() else {
            return Ok(());
        };
        Err(ConfigError::parse(
            named(env, key, material),
            format!(
                "is set beside {declared_by}, which declares a connection in plaintext, where no \
                 certificate is checked or presented: declare an encrypted connection in \
                 {declared_by}, or remove {}",
                self.set_spellings(env),
            ),
        ))
    }

    /// Refuse an identity for a client that cannot present one, naming `client`
    /// — the certificate would go unused, and the peer would refuse a client it
    /// required one of.
    pub fn refuse_identity(&self, env: &ConfigService, client: &'static str) -> Result<()> {
        let Some(identity) = &self.identity else {
            return Ok(());
        };
        Err(ConfigError::parse(
            named(env, "TLS_CERT", &identity.cert),
            format!(
                "is set, but {client} cannot present a client certificate, so it would go \
                 unused: remove it and {}",
                env.spellings("TLS_KEY"),
            ),
        ))
    }

    /// The authorities a peer's certificate must chain to, as PEM: those set,
    /// or [`system_authorities`].
    pub fn authorities_pem(&self) -> &[u8] {
        match &self.authorities {
            Some(authorities) => &authorities.bytes,
            None => system_authorities(),
        }
    }

    /// The identity as PEM — the chain, then the key — for a library that takes
    /// it so.
    pub fn identity_pem(&self) -> Option<(&[u8], &[u8])> {
        self.identity.as_ref().map(|identity| {
            (
                identity.cert.bytes.as_slice(),
                identity.key.bytes.as_slice(),
            )
        })
    }

    /// What to change about a handshake `error` failed, naming `env`'s variables:
    /// `peer` is what the client reached (`"Redis"`, `"the JWK Set endpoint"`),
    /// `address` the key of the variable naming where it dials (`"URL"`). rustls's
    /// own reason is left to the error's source.
    pub fn remedy(
        env: &ConfigService,
        peer: &str,
        address: &str,
        error: &(dyn Error + 'static),
    ) -> String {
        use rustls::{AlertDescription as Alert, CertificateError as Certificate, Error as Tls};

        let Some(refused) = negotiation_error(error) else {
            return format!(
                "check {} and the TLS settings beside it",
                env.spellings(address)
            );
        };
        match refused {
            Tls::InvalidCertificate(
                Certificate::Expired
                | Certificate::ExpiredContext { .. }
                | Certificate::NotValidYet
                | Certificate::NotValidYetContext { .. },
            ) => format!(
                "{peer}'s certificate is outside its validity period — renew it, or check this \
                 host's clock"
            ),
            Tls::InvalidCertificate(
                Certificate::NotValidForName | Certificate::NotValidForNameContext { .. },
            ) => format!(
                "{peer}'s certificate names no host matching the one {} dials — a certificate \
                 names its hosts in subjectAltName, and its common name is not read: dial a name \
                 it carries, or reissue it",
                env.var_name(address)
            ),
            Tls::InvalidCertificate(Certificate::UnknownIssuer | Certificate::BadSignature) => {
                format!(
                    "{peer}'s certificate does not chain to an authority this app trusts — install \
                     the authority that signed it in the system's store, or set {} to it, or have \
                     {peer} serve the intermediate certificates that lead to one",
                    env.spellings("TLS_CA_CERT")
                )
            }
            Tls::InvalidCertificate(
                Certificate::InvalidPurpose | Certificate::InvalidPurposeContext { .. },
            ) => format!(
                "{peer}'s certificate is not issued for server authentication — reissue it with \
                 the serverAuth extended key usage"
            ),
            Tls::InvalidCertificate(_) => {
                format!("{peer}'s certificate cannot serve as a server certificate — reissue it")
            }
            Tls::AlertReceived(Alert::ProtocolVersion) | Tls::PeerIncompatible(_) => format!(
                "{peer}'s TLS settings share nothing with this client, which speaks TLS 1.2 and \
                 1.3 with rustls's default cipher suites — check the protocol versions and the \
                 cipher suites {peer} allows"
            ),
            Tls::AlertReceived(Alert::HandshakeFailure) => format!(
                "{peer} refused the handshake — it requires a client certificate, set through {} \
                 and {}, or shares no cipher suite, key-exchange group or certificate key type \
                 with this client (the cipher suites it allows for TLS 1.2 and for TLS 1.3, and \
                 the key its certificate holds)",
                env.spellings("TLS_CERT"),
                env.spellings("TLS_KEY")
            ),
            Tls::AlertReceived(_) => format!(
                "{peer} refused the handshake — if it requires a client certificate, set {} and \
                 {} to one it accepts",
                env.spellings("TLS_CERT"),
                env.spellings("TLS_KEY")
            ),
            _ => format!(
                "the peer did not complete a TLS handshake — check that the port {} dials serves \
                 TLS",
                env.var_name(address)
            ),
        }
    }

    /// Whether `error` is a TLS negotiation every attempt would fail the same
    /// way — a certificate the client does not accept, a handshake the peer
    /// refused, a peer answering in something other than TLS. An internal error
    /// the peer reports, and a failure of this host's own TLS stack, may clear.
    pub fn negotiation_failed(error: &(dyn Error + 'static)) -> bool {
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

    /// The first piece of material set, by the key it is read under.
    fn first_set(&self) -> Option<(&'static str, &Material)> {
        self.authorities
            .as_ref()
            .map(|authorities| ("TLS_CA_CERT", authorities))
            .or_else(|| {
                self.identity
                    .as_ref()
                    .map(|identity| ("TLS_CERT", &identity.cert))
            })
    }

    /// Every setting holding material, both spellings each.
    fn set_spellings(&self, env: &ConfigService) -> String {
        let mut set = Vec::new();
        if self.authorities.is_some() {
            set.push(env.spellings("TLS_CA_CERT"));
        }
        if self.identity.is_some() {
            set.push(env.spellings("TLS_CERT"));
            set.push(env.spellings("TLS_KEY"));
        }
        set.join(" and ")
    }
}

/// The process-wide rustls provider, aws-lc-rs installed first when the app
/// has not chosen one: the libraries the framework dials through (`redis`,
/// `reqwest`) build from the process default, which rustls picks on its own
/// only when exactly one provider is compiled in — with two, the first
/// handshake panics.
pub fn crypto_provider() -> Arc<CryptoProvider> {
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

/// Why a PEM pair cannot be presented, each half judged as rustls judges it.
#[derive(Debug)]
#[non_exhaustive]
pub enum PairRefusal {
    /// The certificate does not parse as PEM.
    CertificateNotPem,
    /// The certificate holds no CERTIFICATE block.
    NoCertificate,
    /// The key does not parse as PEM.
    KeyNotPem,
    /// The key holds no PRIVATE KEY block.
    NoPrivateKey,
    /// The key provider cannot sign with the key.
    KeyUnusable(rustls::Error),
    /// The certificate is not the key's.
    Mismatch,
    /// The certificate cannot be read as X.509.
    Unreadable(rustls::Error),
}

/// A certificate chain and the key it certifies, built from PEM through `keys`
/// and refused when it cannot be presented: the one judgement of "the
/// certificate is the key's", for a client's identity and a server's
/// certificate alike. A key whose provider hides its public half is accepted,
/// as rustls accepts it.
#[expect(
    clippy::map_err_ignore,
    reason = "the PEM parser's error quotes the line it choked on, which may hold the private key"
)]
pub fn certified_key(
    cert: &[u8],
    key: &[u8],
    keys: &dyn KeyProvider,
) -> std::result::Result<CertifiedKey, PairRefusal> {
    let chain = CertificateDer::pem_slice_iter(cert)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| PairRefusal::CertificateNotPem)?;
    if chain.is_empty() {
        return Err(PairRefusal::NoCertificate);
    }
    let key = PrivateKeyDer::from_pem_slice(key).map_err(|error| match error {
        rustls::pki_types::pem::Error::NoItemsFound => PairRefusal::NoPrivateKey,
        _ => PairRefusal::KeyNotPem,
    })?;
    let signing = keys
        .load_private_key(key)
        .map_err(PairRefusal::KeyUnusable)?;
    let certified = CertifiedKey::new(chain, signing);
    match certified.keys_match() {
        Ok(()) | Err(rustls::Error::InconsistentKeys(rustls::InconsistentKeys::Unknown)) => {
            Ok(certified)
        }
        Err(rustls::Error::InconsistentKeys(_)) => Err(PairRefusal::Mismatch),
        Err(other) => Err(PairRefusal::Unreadable(other)),
    }
}

impl PairRefusal {
    /// The refusal of `identity`, naming `env`'s variable for the half refused.
    fn naming(self, env: &ConfigService, identity: &TlsIdentity) -> ConfigError {
        let cert = || named(env, "TLS_CERT", &identity.cert);
        let key = || named(env, "TLS_KEY", &identity.key);
        match self {
            Self::CertificateNotPem => ConfigError::parse(cert(), NOT_PEM),
            Self::NoCertificate => ConfigError::parse(cert(), "holds no CERTIFICATE block"),
            Self::KeyNotPem => ConfigError::parse(key(), NOT_PEM),
            Self::NoPrivateKey => ConfigError::parse(
                key(),
                "holds no PRIVATE KEY block — an empty value, a certificate or a public key in \
                 its place, or an encrypted key, which has to be decrypted first",
            ),
            Self::KeyUnusable(error) => {
                ConfigError::parse(key(), format!("is not a key rustls can sign with: {error}"))
            }
            Self::Mismatch => ConfigError::parse(
                cert(),
                format!(
                    "is not the certificate of the key in {} — the chain has to start with that \
                     key's own certificate",
                    key()
                ),
            ),
            Self::Unreadable(error) => ConfigError::parse(
                cert(),
                format!("cannot be read as an X.509 certificate: {error}"),
            ),
        }
    }
}

/// What a PEM value has to look like, said instead of quoting one that does not.
const NOT_PEM: &str = "is not valid PEM: each -----BEGIN and -----END line stands on a line of \
     its own, with intact base64 between them, and a value whose line breaks were collapsed or \
     escaped does not parse";

/// The variable `material` was read from under `key`: its `_FILE` spelling
/// when a file was read, the inline one otherwise — what a pin in code is
/// refused under too.
fn named(env: &ConfigService, key: &str, material: &Material) -> String {
    if material.path.is_some() {
        env.var_name(&format!("{key}_FILE"))
    } else {
        env.var_name(key)
    }
}

fn half_an_identity(env: &ConfigService, given: &Setting<Material>, missing: &str) -> ConfigError {
    given.refuse(format_args!(
        "is set without {} — a client certificate is presented with its key",
        env.spellings(missing),
    ))
}

/// The rustls error a failed negotiation carries, when `error` is one.
fn negotiation_error<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a rustls::Error> {
    let mut next = Some(error);
    while let Some(current) = next {
        if let Some(refused) = current.downcast_ref::<rustls::Error>() {
            return Some(refused);
        }
        next = held(current).or_else(|| current.source());
    }
    None
}

/// The error `error` holds where its own `source` skips it: behind an `Arc`
/// (`redis`), whose `source` is the held error's own, and inside an io error,
/// which is how tokio-rustls hands a failed handshake back.
fn held<'a>(error: &'a (dyn Error + 'static)) -> Option<&'a (dyn Error + 'static)> {
    if let Some(shared) = error.downcast_ref::<Arc<dyn Error + Send + Sync>>() {
        return Some(&**shared);
    }
    error
        .downcast_ref::<io::Error>()
        .and_then(io::Error::get_ref)
        .map(|inner| inner as &(dyn Error + 'static))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::LazyLock;

    use nest_rs_testing::{TestAuthority, TestCertificate};

    static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

    /// A client certificate and its key, issued for this test process.
    static CLIENT: LazyLock<TestCertificate> =
        LazyLock::new(|| AUTHORITY.client("nestrs-test-client"));

    /// Another client's, whose key is not `CLIENT`'s.
    static OTHER: LazyLock<TestCertificate> =
        LazyLock::new(|| AUTHORITY.client("nestrs-other-client"));

    fn inline(pem: &[u8]) -> Material {
        Material {
            bytes: pem.to_vec(),
            path: None,
        }
    }

    fn presenting(cert: &[u8], key: &[u8]) -> ClientTls {
        ClientTls::new(None, Some(TlsIdentity::new(inline(cert), inline(key))))
    }

    fn redis() -> ConfigService {
        ConfigService::with_vars("redis", [])
    }

    fn var(key: &str) -> String {
        crate::var_name("redis", key)
    }

    fn refusal(tls: &ClientTls) -> String {
        tls.check(&redis())
            .expect_err("the material is refused")
            .to_string()
    }

    #[test]
    fn the_deployment_overlays_each_field_and_the_certificate_moves_with_its_key() {
        let pinned = ClientTls::new(
            Some(inline(b"pinned-authority")),
            Some(TlsIdentity::new(
                inline(b"pinned-cert"),
                inline(b"pinned-key"),
            )),
        );
        let tls = ClientTls::from_env(
            &ConfigService::with_vars("redis", [("TLS_CA_CERT", "deployed-authority")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(
            tls.authorities_pem(),
            b"deployed-authority",
            "the deployment outranks the pin"
        );
        assert_eq!(
            tls.identity_pem(),
            Some((&b"pinned-cert"[..], &b"pinned-key"[..])),
            "and the untouched pair survives"
        );

        let replaced = ClientTls::from_env(
            &ConfigService::with_vars(
                "redis",
                [("TLS_CERT", "deployed-cert"), ("TLS_KEY", "deployed-key")],
            ),
            ClientTls::default(),
        )
        .expect("a whole pair resolves");
        assert_eq!(
            replaced.identity_pem(),
            Some((&b"deployed-cert"[..], &b"deployed-key"[..]))
        );
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test tears down its temp file best-effort"
    )]
    fn half_a_client_certificate_is_refused_naming_what_was_set_and_what_is_missing() {
        let path = std::env::temp_dir().join(format!(
            "nest-rs-config-tls-half-{}.pem",
            std::process::id()
        ));
        std::fs::write(&path, CLIENT.cert.as_bytes()).expect("write the certificate");
        let file = path.to_str().expect("a UTF-8 temporary path");
        let read = [
            ("TLS_CERT", "-----PEM-----", "TLS_CERT", "TLS_KEY"),
            ("TLS_KEY", "-----PEM-----", "TLS_KEY", "TLS_CERT"),
            ("TLS_CERT_FILE", file, "TLS_CERT_FILE", "TLS_KEY"),
        ]
        .map(|(key, value, named, missing)| {
            let outcome = ClientTls::from_env(
                &ConfigService::with_vars("redis", [(key, value)]),
                ClientTls::default(),
            );
            (outcome, named, missing)
        });
        let _ = std::fs::remove_file(&path);

        for (outcome, named, missing) in read {
            let err = outcome.expect_err("half a pair is refused");
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

    #[test]
    fn a_named_instance_reads_and_names_its_own_namespace() {
        let cache = ConfigService::with_vars("redis__cache", [("TLS_CA_CERT", "no certificate")]);
        let tls = ClientTls::from_env(&cache, ClientTls::default()).expect("the overlay resolves");
        let refused = tls.check(&cache).expect_err("no certificate").to_string();
        assert!(
            refused.contains(&crate::var_name("redis__cache", "TLS_CA_CERT")),
            "{refused}"
        );
    }

    #[test]
    fn nothing_set_trusts_the_system_and_presents_nothing() {
        let tls =
            ClientTls::from_env(&redis(), ClientTls::default()).expect("nothing set resolves");
        assert!(tls.is_empty());
        assert_eq!(tls.authorities_pem(), system_authorities());
        assert!(tls.identity_pem().is_none());
        tls.check(&redis())
            .expect("the system's store is not judged");
    }

    #[test]
    fn authorities_no_handshake_could_use_are_refused_naming_their_variable() {
        for (pem, says) in [
            (&b"not a certificate s3cret"[..], "no CERTIFICATE block"),
            (b"", "no CERTIFICATE block"),
            (
                b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
                "cannot use as an authority",
            ),
        ] {
            let refused = refusal(&ClientTls::new(Some(inline(pem)), None));
            assert!(
                refused.contains(&var("TLS_CA_CERT")) && refused.contains(says),
                "{refused}"
            );
            assert!(!refused.contains("s3cret"), "{refused}");
        }
        ClientTls::new(Some(inline(AUTHORITY.pem().as_bytes())), None)
            .check(&redis())
            .expect("an authority is accepted");
    }

    #[test]
    fn material_read_from_a_file_is_refused_under_its_file_variable() {
        let tls = ClientTls::new(
            Some(Material {
                bytes: b"no certificate".to_vec(),
                path: Some("/run/secrets/ca.pem".into()),
            }),
            None,
        );
        let refused = refusal(&tls);
        assert!(
            refused.contains(&format!("{}:", var("TLS_CA_CERT_FILE"))),
            "{refused}"
        );
    }

    #[test]
    fn a_certificate_that_is_not_the_keys_is_refused() {
        let refused = refusal(&presenting(CLIENT.cert.as_bytes(), OTHER.key.as_bytes()));
        assert!(
            refused.contains(&var("TLS_CERT"))
                && refused.contains(&var("TLS_KEY"))
                && refused.contains("is not the certificate of the key"),
            "{refused}"
        );
        presenting(CLIENT.cert.as_bytes(), CLIENT.key.as_bytes())
            .check(&redis())
            .expect("a certificate and its own key are accepted");
    }

    #[test]
    fn a_key_setting_holding_no_private_key_says_so() {
        let encrypted = CLIENT
            .key
            .replace("BEGIN PRIVATE KEY", "BEGIN ENCRYPTED PRIVATE KEY")
            .replace("END PRIVATE KEY", "END ENCRYPTED PRIVATE KEY");
        for (label, key) in [
            ("empty", String::new()),
            ("a certificate", CLIENT.cert.clone()),
            ("an encrypted key", encrypted),
        ] {
            let refused = refusal(&presenting(CLIENT.cert.as_bytes(), key.as_bytes()));
            assert!(
                refused.contains("no PRIVATE KEY block") && refused.contains("decrypted first"),
                "{label}: {refused}"
            );
        }
        let refused = refusal(&presenting(b"", CLIENT.key.as_bytes()));
        assert!(
            refused.contains(&var("TLS_CERT")) && refused.contains("no CERTIFICATE block"),
            "{refused}"
        );
    }

    #[test]
    fn material_that_does_not_parse_is_described_and_never_quoted() {
        let secret = CLIENT.key.lines().nth(1).expect("a base64 line of the key");
        let as_bytes = secret.as_bytes()[..4]
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for collapsed in [
            CLIENT.key.replace('\n', " "),
            CLIENT.key.replace('\n', "\\n"),
        ] {
            let refused = refusal(&presenting(CLIENT.cert.as_bytes(), collapsed.as_bytes()));
            assert!(
                refused.contains(&var("TLS_KEY")),
                "names the setting: {refused}"
            );
            assert!(
                !refused.contains(&secret[..12]),
                "never quotes the key: {refused}"
            );
            assert!(!refused.contains(&as_bytes), "not even as bytes: {refused}");
        }
    }

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
        impl KeyProvider for OpaqueKeys {
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
        assert!(
            certified_key(CLIENT.cert.as_bytes(), OTHER.key.as_bytes(), &OpaqueKeys).is_ok(),
            "rustls itself cannot tell, and presents it"
        );
    }

    #[test]
    fn material_beside_a_plaintext_connection_is_refused_naming_what_declares_it() {
        let declared_by = var("URL");
        ClientTls::default()
            .refuse_beside_plaintext(&redis(), &declared_by)
            .expect("nothing set goes unused");
        for tls in [
            ClientTls::new(Some(inline(AUTHORITY.pem().as_bytes())), None),
            presenting(CLIENT.cert.as_bytes(), CLIENT.key.as_bytes()),
        ] {
            let refused = tls
                .refuse_beside_plaintext(&redis(), &declared_by)
                .expect_err("material beside plaintext goes unused")
                .to_string();
            assert!(
                refused.contains(&declared_by) && refused.contains("plaintext"),
                "{refused}"
            );
        }
    }

    #[test]
    fn an_identity_for_a_client_that_cannot_present_one_is_refused_naming_the_client() {
        ClientTls::new(Some(inline(AUTHORITY.pem().as_bytes())), None)
            .refuse_identity(&redis(), "object_store")
            .expect("authorities alone are presented by no one");
        let refused = presenting(CLIENT.cert.as_bytes(), CLIENT.key.as_bytes())
            .refuse_identity(&redis(), "object_store")
            .expect_err("an identity no one presents")
            .to_string();
        assert!(
            refused.contains(&var("TLS_CERT"))
                && refused.contains(&var("TLS_KEY"))
                && refused.contains("object_store"),
            "{refused}"
        );
    }

    #[test]
    fn the_process_provider_is_installed_once_and_handed_back() {
        let provider = crypto_provider();
        let installed = CryptoProvider::get_default().expect("a provider is installed");
        assert!(
            Arc::ptr_eq(&provider, installed),
            "and it is the one handed back"
        );
        assert!(Arc::ptr_eq(&crypto_provider(), installed), "every time");
    }

    fn negotiation(refused: rustls::Error) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, refused)
    }

    #[test]
    fn a_refused_handshake_is_told_apart_from_what_may_clear() {
        let refused = negotiation(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        ));
        let internal = negotiation(rustls::Error::AlertReceived(
            rustls::AlertDescription::InternalError,
        ));
        let dropped = io::Error::from(io::ErrorKind::ConnectionReset);
        let local = negotiation(rustls::Error::FailedToGetCurrentTime);
        let rejected_here = negotiation(rustls::Error::General("no usable configuration".into()));
        let shared: Arc<dyn Error + Send + Sync> = Arc::new(negotiation(
            rustls::Error::InvalidCertificate(rustls::CertificateError::Expired),
        ));

        assert!(ClientTls::negotiation_failed(&refused), "{refused}");
        assert!(
            ClientTls::negotiation_failed(&shared),
            "behind an Arc: {shared}"
        );
        let rewrapped = io::Error::other(negotiation(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        )));
        assert!(
            ClientTls::negotiation_failed(&rewrapped),
            "an io error around one, as reqwest hands it back: {rewrapped}"
        );
        assert!(!ClientTls::negotiation_failed(&internal), "{internal}");
        assert!(!ClientTls::negotiation_failed(&dropped), "{dropped}");
        assert!(!ClientTls::negotiation_failed(&local), "{local}");
        assert!(
            !ClientTls::negotiation_failed(&rejected_here),
            "{rejected_here}"
        );
    }

    #[test]
    fn a_refusal_names_the_setting_that_fixes_its_cause() {
        use rustls::{AlertDescription, CertificateError, Error, InvalidMessage, PeerIncompatible};

        for (refused, says, not) in [
            (
                Error::InvalidCertificate(CertificateError::UnknownIssuer),
                format!("set {}", crate::spellings("redis", "TLS_CA_CERT")),
                var("URL"),
            ),
            (
                Error::InvalidCertificate(CertificateError::BadSignature),
                format!("set {}", crate::spellings("redis", "TLS_CA_CERT")),
                var("URL"),
            ),
            (
                Error::InvalidCertificate(CertificateError::NotValidForName),
                format!("{} dials", var("URL")),
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
                "protocol versions".to_owned(),
                var("TLS_KEY"),
            ),
            (
                Error::PeerIncompatible(PeerIncompatible::NoCipherSuitesInCommon),
                "protocol versions".to_owned(),
                var("TLS_KEY"),
            ),
            (
                Error::AlertReceived(AlertDescription::HandshakeFailure),
                "cipher suites it allows for TLS 1.2".to_owned(),
                var("TLS_CA_CERT"),
            ),
            (
                Error::AlertReceived(AlertDescription::CertificateRequired),
                format!("set {}", crate::spellings("redis", "TLS_CERT")),
                "cipher suites".to_owned(),
            ),
            (
                Error::InvalidMessage(InvalidMessage::InvalidContentType),
                "serves TLS".to_owned(),
                "subjectAltName".to_owned(),
            ),
        ] {
            let reason = refused.to_string();
            let sentence = ClientTls::remedy(&redis(), "Redis", "URL", &negotiation(refused));
            assert!(sentence.contains(&says), "says {says}: {sentence}");
            assert!(!sentence.contains(&not), "and not {not}: {sentence}");
            assert!(
                !sentence.contains(&reason),
                "and leaves rustls's reason to the source: {sentence}"
            );
        }
        let elsewhere = ClientTls::remedy(
            &ConfigService::with_vars("authn", []),
            "the JWK Set endpoint",
            "JWKS_URI",
            &negotiation(Error::InvalidCertificate(CertificateError::NotValidForName)),
        );
        assert!(
            elsewhere.starts_with("the JWK Set endpoint's certificate")
                && elsewhere.contains(&crate::var_name("authn", "JWKS_URI")),
            "the peer and the setting are the caller's: {elsewhere}"
        );
    }

    #[test]
    fn debug_never_shows_the_private_key() {
        let tls = presenting(b"cert", b"s3cr3t");
        let shown = format!("{tls:?}");
        assert!(!shown.contains("s3cr3t"), "{shown}");
    }
}
