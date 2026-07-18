use std::path::Path;

use anyhow::{Context, Result};
use nest_rs_config::env_var;
use poem::listener::{RustlsCertificate, RustlsConfig};

/// TLS material for the HTTP transport: a PEM certificate chain and private
/// key, handed to [`HttpTransport::tls`](crate::HttpTransport::tls).
///
/// ```no_run
/// # use nest_rs_http::{HttpTransport, TlsConfig};
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
    /// Build a TLS config from PEM certificate and private-key bytes.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_round_trips_bytes() {
        let cfg = TlsConfig::new(b"--CERT--".to_vec(), b"--KEY--".to_vec());
        assert_eq!(cfg.cert, b"--CERT--");
        assert_eq!(cfg.key, b"--KEY--");
    }

    // The derived `Debug` on `HttpConfig` would leak the key into logs — this
    // test pins the manual impl that redacts both the cert and key bytes.
    #[test]
    fn debug_redacts_key_bytes() {
        let cfg = TlsConfig::new(vec![0; 128], b"super secret key material".to_vec());
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("super secret"), "key leaked: {debug}");
        assert!(
            debug.contains("redacted"),
            "missing redaction marker: {debug}"
        );
        assert!(debug.contains("128 bytes"), "cert length missing: {debug}");
    }

    // `TlsConfig::from_env` reads the `NESTRS_HTTP__TLS_*` keys through the
    // framework-wide `env_var` shortcut (real process env + `.env` cascade), not
    // a `ConfigService` — so a map-backed source can't feed it. Isolate the real
    // env with `figment::Jail` (the same approach `nest-rs-config` uses for its
    // own `env_var` tests), which needs no `unsafe` in this crate and reverts
    // every mutation when the closure returns.

    #[test]
    #[allow(clippy::result_large_err)]
    fn from_env_is_none_when_no_tls_vars_are_set() {
        figment::Jail::expect_with(|_| {
            assert!(TlsConfig::from_env().expect("no error").is_none());
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn from_env_reads_inline_pem_pair() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_HTTP__TLS_CERT", "--CERT--");
            jail.set_env("NESTRS_HTTP__TLS_KEY", "--KEY--");
            let cfg = TlsConfig::from_env().expect("no error").expect("Some");
            assert_eq!(cfg.cert, b"--CERT--");
            assert_eq!(cfg.key, b"--KEY--");
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn from_env_fails_when_only_cert_is_set() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_HTTP__TLS_CERT", "--CERT--");
            let err = TlsConfig::from_env().expect_err("half-config is rejected");
            let msg = err.to_string();
            assert!(msg.contains("KEY"), "must name the missing var: {msg}");
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn from_env_fails_when_only_key_is_set() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_HTTP__TLS_KEY", "--KEY--");
            let err = TlsConfig::from_env().expect_err("half-config is rejected");
            let msg = err.to_string();
            assert!(msg.contains("CERT"), "must name the missing var: {msg}");
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn from_env_reads_file_variants_when_inline_unset() {
        figment::Jail::expect_with(|jail| {
            // `Jail` runs in a fresh temp CWD; write the PEM files there and point
            // the `_FILE` vars at them by relative path.
            jail.create_file("cert.pem", "file-cert-bytes")?;
            jail.create_file("key.pem", "file-key-bytes")?;
            jail.set_env("NESTRS_HTTP__TLS_CERT_FILE", "cert.pem");
            jail.set_env("NESTRS_HTTP__TLS_KEY_FILE", "key.pem");
            let cfg = TlsConfig::from_env().expect("no error").expect("Some");
            assert_eq!(cfg.cert, b"file-cert-bytes");
            assert_eq!(cfg.key, b"file-key-bytes");
            Ok(())
        });
    }
}
