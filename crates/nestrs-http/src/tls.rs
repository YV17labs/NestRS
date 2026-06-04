use std::path::Path;

use anyhow::{Context, Result};
use nestrs_config::env_var;
use poem::listener::{RustlsCertificate, RustlsConfig};

/// TLS material for the HTTP transport: a PEM certificate chain and private
/// key, handed to [`HttpTransport::tls`](crate::HttpTransport::tls).
///
/// ```no_run
/// # use nestrs_http::{HttpTransport, TlsConfig};
/// let mut http = HttpTransport::new().bind("0.0.0.0:3000");
/// if let Some(tls) = TlsConfig::from_env()? {
///     http = http.tls(tls);
/// }
/// # Ok::<(), anyhow::Error>(())
/// ```
#[derive(Clone)]
pub struct TlsConfig {
    cert: Vec<u8>,
    key: Vec<u8>,
}

/// Manual `Debug` so `HttpConfig`'s derived `Debug` cannot leak the private
/// key to a log line — only sizes are printed.
impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsConfig")
            .field("cert", &format_args!("<{} bytes>", self.cert.len()))
            .field("key", &format_args!("<{} bytes redacted>", self.key.len()))
            .finish()
    }
}

impl TlsConfig {
    pub fn new(cert: impl Into<Vec<u8>>, key: impl Into<Vec<u8>>) -> Self {
        Self {
            cert: cert.into(),
            key: key.into(),
        }
    }

    /// Read TLS material from `NESTRS_HTTP__TLS_CERT` / `NESTRS_HTTP__TLS_KEY`
    /// (PEM inline) or their `_FILE` variants (path the transport loads); the
    /// inline form wins if both are set.
    ///
    /// `Ok(None)` when neither is present (serve plain HTTP). Fails if exactly
    /// one of the pair is configured — a half-configured TLS is a deployment
    /// mistake, not a silent fall back to plaintext.
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
