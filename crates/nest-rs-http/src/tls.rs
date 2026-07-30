use std::path::Path;

use anyhow::{Context, Result};
use nest_rs_config::ConfigService;
use poem::listener::{RustlsCertificate, RustlsConfig};

/// TLS material for the HTTP transport: a PEM certificate chain and private
/// key, handed to [`HttpTransport::tls`](crate::HttpTransport::tls).
///
/// ```no_run
/// # use nest_rs_config::ConfigService;
/// # use nest_rs_http::{HttpTransport, TlsConfig};
/// let env = ConfigService::for_namespace("http");
/// let mut http = HttpTransport::new().bind("0.0.0.0:3000");
/// if let Some(tls) = TlsConfig::from_env(&env, None)? {
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
    /// inline form wins if both are set. `base` is what the field keeps when the
    /// environment configures neither half.
    ///
    /// `Ok(None)` when neither is present and `base` is `None` (serve plain
    /// HTTP). Fails if exactly one of the pair is configured — a
    /// half-configured TLS is a deployment mistake, not a silent fall back to
    /// plaintext.
    ///
    /// Reads through the [`ConfigService`] rather than the process env directly
    /// so a pinned `HttpConfig` and a deployment variable resolve on the same
    /// precedence ladder as every other field.
    pub fn from_env(env: &ConfigService, base: Option<Self>) -> Result<Option<Self>> {
        let cert = read_env_pem(env, "TLS_CERT", "TLS_CERT_FILE")?;
        let key = read_env_pem(env, "TLS_KEY", "TLS_KEY_FILE")?;
        match (cert, key) {
            (Some(cert), Some(key)) => Ok(Some(Self::new(cert, key))),
            (None, None) => Ok(base),
            (Some(_), None) => anyhow::bail!(
                "{} is set but no key ({} / _FILE)",
                env.var_name("TLS_CERT"),
                env.var_name("TLS_KEY"),
            ),
            (None, Some(_)) => anyhow::bail!(
                "{} is set but no certificate ({} / _FILE)",
                env.var_name("TLS_KEY"),
                env.var_name("TLS_CERT"),
            ),
        }
    }

    pub(crate) fn into_rustls(self) -> RustlsConfig {
        RustlsConfig::new().fallback(RustlsCertificate::new().cert(self.cert).key(self.key))
    }
}

fn read_env_pem(env: &ConfigService, inline_key: &str, file_key: &str) -> Result<Option<Vec<u8>>> {
    if let Some(pem) = env.get(inline_key) {
        return Ok(Some(pem.into_bytes()));
    }
    match env.get(file_key) {
        Some(path) => {
            let bytes = std::fs::read(Path::new(&path))
                .with_context(|| format!("reading {} at {path}", env.var_name(file_key)))?;
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

    // `TlsConfig::from_env` resolves through a `ConfigService`, so a map-backed
    // source feeds it hermetically — no process-env mutation, no `unsafe`, safe
    // under parallel test execution. Only the `_FILE` variant still needs a real
    // working directory, and `figment::Jail` supplies one.

    fn tls_env<'a>(vars: impl IntoIterator<Item = (&'a str, &'a str)>) -> ConfigService {
        ConfigService::with_vars("http", vars)
    }

    #[test]
    fn from_env_is_none_when_no_tls_vars_are_set() {
        assert!(
            TlsConfig::from_env(&tls_env([]), None)
                .expect("no error")
                .is_none()
        );
    }

    #[test]
    fn from_env_keeps_the_base_when_no_tls_vars_are_set() {
        // The pinned-config path: an `HttpConfig` carrying TLS in code must not
        // lose it just because the environment says nothing about TLS.
        let base = TlsConfig::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        let kept = TlsConfig::from_env(&tls_env([]), Some(base))
            .expect("no error")
            .expect("the base survives an env that configures no TLS");
        assert_eq!(kept.cert, b"--PINNED-CERT--");
    }

    #[test]
    fn from_env_overrides_the_base_when_both_halves_are_set() {
        let base = TlsConfig::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        let cfg = TlsConfig::from_env(
            &tls_env([
                ("NESTRS_HTTP__TLS_CERT", "--ENV-CERT--"),
                ("NESTRS_HTTP__TLS_KEY", "--ENV-KEY--"),
            ]),
            Some(base),
        )
        .expect("no error")
        .expect("Some");
        assert_eq!(cfg.cert, b"--ENV-CERT--");
        assert_eq!(cfg.key, b"--ENV-KEY--");
    }

    #[test]
    fn from_env_reads_inline_pem_pair() {
        let cfg = TlsConfig::from_env(
            &tls_env([
                ("NESTRS_HTTP__TLS_CERT", "--CERT--"),
                ("NESTRS_HTTP__TLS_KEY", "--KEY--"),
            ]),
            None,
        )
        .expect("no error")
        .expect("Some");
        assert_eq!(cfg.cert, b"--CERT--");
        assert_eq!(cfg.key, b"--KEY--");
    }

    #[test]
    fn from_env_fails_when_only_cert_is_set() {
        let err = TlsConfig::from_env(&tls_env([("NESTRS_HTTP__TLS_CERT", "--CERT--")]), None)
            .expect_err("half-config is rejected");
        let msg = err.to_string();
        assert!(msg.contains("KEY"), "must name the missing var: {msg}");
    }

    #[test]
    fn from_env_fails_when_only_key_is_set() {
        let err = TlsConfig::from_env(&tls_env([("NESTRS_HTTP__TLS_KEY", "--KEY--")]), None)
            .expect_err("half-config is rejected");
        let msg = err.to_string();
        assert!(msg.contains("CERT"), "must name the missing var: {msg}");
    }

    // A half-configured TLS is refused even when the base already carries a
    // complete pair: the deployment clearly meant to swap the material, and
    // silently serving the pinned cert instead would hide the mistake.
    #[test]
    fn from_env_fails_on_a_half_config_even_over_a_complete_base() {
        let base = TlsConfig::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        assert!(
            TlsConfig::from_env(
                &tls_env([("NESTRS_HTTP__TLS_CERT", "--ENV-CERT--")]),
                Some(base),
            )
            .is_err()
        );
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn from_env_reads_file_variants_when_inline_unset() {
        figment::Jail::expect_with(|jail| {
            // `Jail` runs in a fresh temp CWD; write the PEM files there and point
            // the `_FILE` vars at them by relative path.
            jail.create_file("cert.pem", "file-cert-bytes")?;
            jail.create_file("key.pem", "file-key-bytes")?;
            let cfg = TlsConfig::from_env(
                &tls_env([
                    ("NESTRS_HTTP__TLS_CERT_FILE", "cert.pem"),
                    ("NESTRS_HTTP__TLS_KEY_FILE", "key.pem"),
                ]),
                None,
            )
            .expect("no error")
            .expect("Some");
            assert_eq!(cfg.cert, b"file-cert-bytes");
            assert_eq!(cfg.key, b"file-key-bytes");
            Ok(())
        });
    }
}
