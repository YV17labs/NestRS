//! [`AuthnTls`] — what the JWK Set endpoint's certificate must chain to.

use nest_rs_config::{ConfigService, Result};

/// TLS trust for the JWK Set endpoint `<PREFIX>_AUTHN__JWKS_URI` names. The
/// default — nothing set — trusts the system's authorities; the certificate is
/// verified either way, and its name checked against the URI's host.
///
/// Read once, when the configuration resolves: a renewed file takes effect at
/// the next boot.
#[derive(Clone, Default)]
pub struct AuthnTls {
    /// PEM certificates of the authorities the endpoint's certificate must
    /// chain to, **replacing** the system's — for an issuer a private authority
    /// signed. Read from `<PREFIX>_AUTHN__TLS_CA_CERT`, or the file
    /// `<PREFIX>_AUTHN__TLS_CA_CERT_FILE` names.
    pub ca_cert: Option<Vec<u8>>,
}

impl AuthnTls {
    /// Overlay the deployment's TLS variable on `base`. Whether the material
    /// holds a certificate is [`JwtService::new`](crate::JwtService::new)'s to
    /// judge, the one place every path reaches.
    pub(crate) fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            ca_cert: match env.material("TLS_CA_CERT")? {
                Some(pem) => Some(pem.value.bytes),
                None => base.ca_cert,
            },
        })
    }
}
