//! [`TestAuthority`] — a certificate authority of a test's own, issuing in
//! process the certificates a TLS test presents or trusts, so none is
//! committed.

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};

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
