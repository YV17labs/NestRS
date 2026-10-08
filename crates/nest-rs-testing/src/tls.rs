//! [`TestAuthority`] — a certificate authority of a test's own, issuing in
//! process the certificates a TLS test presents or trusts, so none is
//! committed — and the two handshakes a TLS double runs: the one it accepts
//! with a [`TestCertificate`], and the one it dials a service with
//! ([`system_connector`]).

use std::sync::{Arc, LazyLock};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// A self-signed certificate authority, born with the test that makes it: a
/// TLS double presents what it [issues](Self::server), and the client under
/// test trusts its [`pem`](Self::pem) — or does not, which is a test too.
///
/// ```
/// use nest_rs_testing::TestAuthority;
///
/// let authority = TestAuthority::new();
/// let server = authority.server(&["localhost", "127.0.0.1"]);
/// assert!(authority.pem().starts_with("-----BEGIN CERTIFICATE-----"));
/// assert!(server.key.starts_with("-----BEGIN PRIVATE KEY-----"));
/// ```
pub struct TestAuthority {
    issuer: Issuer<'static, KeyPair>,
    pem: String,
}

/// A certificate a [`TestAuthority`] issued and its private key, both PEM.
#[derive(Clone)]
pub struct TestCertificate {
    /// The certificate, its issuer's not appended.
    pub cert: String,
    /// The private key it certifies, PKCS #8.
    pub key: String,
}

impl Default for TestAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl TestAuthority {
    /// A new authority, its key on a P-256 curve.
    pub fn new() -> Self {
        let key = KeyPair::generate().expect("a P-256 key generates");
        let mut params =
            CertificateParams::new(Vec::<String>::new()).expect("an authority names no host");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params
            .distinguished_name
            .push(DnType::CommonName, "nestrs test authority");
        let pem = params
            .self_signed(&key)
            .expect("the authority signs itself")
            .pem();
        Self {
            issuer: Issuer::new(params, key),
            pem,
        }
    }

    /// The authority's certificate, for a client to trust.
    pub fn pem(&self) -> &str {
        &self.pem
    }

    /// A server certificate naming `names` — each a DNS name, or an IP
    /// address when it parses as one.
    pub fn server(&self, names: &[&str]) -> TestCertificate {
        self.issue(names, ExtendedKeyUsagePurpose::ServerAuth)
    }

    /// A client certificate for `name`, to present to a server requiring one.
    pub fn client(&self, name: &str) -> TestCertificate {
        self.issue(&[name], ExtendedKeyUsagePurpose::ClientAuth)
    }

    fn issue(&self, names: &[&str], usage: ExtendedKeyUsagePurpose) -> TestCertificate {
        let key = KeyPair::generate().expect("a P-256 key generates");
        let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        let mut params = CertificateParams::new(names.clone()).expect("each name is a host name");
        if let Some(first) = names.first() {
            params.distinguished_name.push(DnType::CommonName, first);
        }
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![usage];
        let cert = params
            .signed_by(&key, &self.issuer)
            .expect("the authority signs the certificate");
        TestCertificate {
            cert: cert.pem(),
            key: key.serialize_pem(),
        }
    }
}

impl TestCertificate {
    /// The server side of a handshake presenting this certificate — asking
    /// every client for a certificate `clients` issued, when it names one, as a
    /// server requiring client certificates does.
    pub fn acceptor(&self, clients: Option<&TestAuthority>) -> TlsAcceptor {
        let provider = provider();
        let builder = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("the provider speaks the default protocol versions");
        let builder = match clients {
            None => builder.with_no_client_auth(),
            Some(clients) => {
                let verifier = WebPkiClientVerifier::builder_with_provider(
                    Arc::new(roots(clients.pem())),
                    provider,
                )
                .build()
                .expect("a client verifier over the authority");
                builder.with_client_cert_verifier(verifier)
            }
        };
        let chain = CertificateDer::pem_slice_iter(self.cert.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("the certificate parses");
        let key = PrivateKeyDer::from_pem_slice(self.key.as_bytes()).expect("the key parses");
        TlsAcceptor::from(Arc::new(
            builder
                .with_single_cert(chain, key)
                .expect("the certificate and key correspond"),
        ))
    }
}

/// The client side of a handshake with a service the system trusts — the
/// development services, whose authority the dev container and CI install in
/// the system's store — built once per process.
pub fn system_connector() -> TlsConnector {
    static CONNECTOR: LazyLock<TlsConnector> = LazyLock::new(|| {
        let config = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .expect("the provider speaks the default protocol versions")
            .with_root_certificates(roots(nest_rs_config::system_authorities()))
            .with_no_client_auth();
        TlsConnector::from(Arc::new(config))
    });
    CONNECTOR.clone()
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// The trust anchors `pem` holds.
fn roots(pem: impl AsRef<[u8]>) -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(
        CertificateDer::pem_slice_iter(pem.as_ref()).filter_map(Result::ok),
    );
    roots
}
