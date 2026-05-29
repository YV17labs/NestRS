use std::path::Path;

use anyhow::{Context, Result};
use nestrs_config::env_var;
use poem::listener::{RustlsCertificate, RustlsConfig};

/// TLS material for the HTTP transport: a PEM certificate chain and its private
/// key. Hand it to [`HttpTransport::tls`](crate::HttpTransport::tls) and the
/// transport serves HTTPS directly (poem's `rustls` listener, no OpenSSL system
/// dependency) instead of plain HTTP.
///
/// In a container deployment (Kubernetes, a service mesh) the certificate and
/// key are *injected* — mounted as files or passed as environment variables — so
/// [`TlsConfig::from_env`] reads them off the framework-wide `NESTRS_HTTP__TLS_*`
/// scheme with no ceremony and returns `None` when they are unset, the dev
/// default that keeps the transport plaintext. `main` then opts in with one line
/// only when the certs are actually present:
///
/// ```no_run
/// # use nestrs_http::{HttpTransport, TlsConfig};
/// let mut http = HttpTransport::new().bind("0.0.0.0:3000");
/// if let Some(tls) = TlsConfig::from_env()? {
///     http = http.tls(tls);
/// }
/// # Ok::<(), anyhow::Error>(())
/// ```
pub struct TlsConfig {
    cert: Vec<u8>,
    key: Vec<u8>,
}

impl TlsConfig {
    /// Build a config from a PEM certificate chain and private key already in
    /// memory.
    pub fn new(cert: impl Into<Vec<u8>>, key: impl Into<Vec<u8>>) -> Self {
        Self {
            cert: cert.into(),
            key: key.into(),
        }
    }

    /// Read TLS material from the framework's `NESTRS_<DOMAIN>__<KEY>` env scheme
    /// (domain `http`), the channel a mesh / orchestrator uses to inject
    /// certificates. For each of the certificate and the key: the inline variable
    /// (`NESTRS_HTTP__TLS_CERT` / `NESTRS_HTTP__TLS_KEY`) holds the PEM directly,
    /// while the `*_FILE` variant (`NESTRS_HTTP__TLS_CERT_FILE` /
    /// `NESTRS_HTTP__TLS_KEY_FILE`) names a path the transport loads — the inline
    /// form wins if both are set.
    ///
    /// Returns `Ok(None)` when **neither** the certificate nor the key is
    /// present (serve plain HTTP), and an `Err` when exactly one of the pair is
    /// configured — a half-configured TLS is a deployment mistake, not a silent
    /// fall back to plaintext.
    pub fn from_env() -> Result<Option<Self>> {
        let cert = read_env_pem("NESTRS_HTTP__TLS_CERT", "NESTRS_HTTP__TLS_CERT_FILE")?;
        let key = read_env_pem("NESTRS_HTTP__TLS_KEY", "NESTRS_HTTP__TLS_KEY_FILE")?;
        match (cert, key) {
            (Some(cert), Some(key)) => Ok(Some(Self::new(cert, key))),
            (None, None) => Ok(None),
            (Some(_), None) => anyhow::bail!(
                "NESTRS_HTTP__TLS_CERT is set but no key (NESTRS_HTTP__TLS_KEY / _FILE)"
            ),
            (None, Some(_)) => anyhow::bail!(
                "NESTRS_HTTP__TLS_KEY is set but no certificate (NESTRS_HTTP__TLS_CERT / _FILE)"
            ),
        }
    }

    pub(crate) fn into_rustls(self) -> RustlsConfig {
        RustlsConfig::new().fallback(RustlsCertificate::new().cert(self.cert).key(self.key))
    }
}

/// Read a PEM blob from `inline_var` (the value *is* the PEM), falling back to
/// the path in `file_var`. `None` when neither is set.
fn read_env_pem(inline_var: &str, file_var: &str) -> Result<Option<Vec<u8>>> {
    if let Some(pem) = env_var(inline_var) {
        return Ok(Some(pem.into_bytes()));
    }
    match env_var(file_var) {
        Some(path) => {
            let bytes = std::fs::read(Path::new(&path))
                .with_context(|| format!("reading {file_var} at {path}"))?;
            Ok(Some(bytes))
        }
        None => Ok(None),
    }
}
