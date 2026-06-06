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
        assert!(debug.contains("redacted"), "missing redaction marker: {debug}");
        assert!(debug.contains("128 bytes"), "cert length missing: {debug}");
    }

    // Real env mutation; serialize so tests in this binary don't race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in vars {
            match v {
                Some(value) => unsafe { std::env::set_var(k, value) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        let out = f();
        for (k, _) in vars {
            unsafe { std::env::remove_var(k) };
        }
        out
    }

    #[test]
    fn from_env_is_none_when_no_tls_vars_are_set() {
        with_env(
            &[
                ("NESTRS_HTTP__TLS_CERT", None),
                ("NESTRS_HTTP__TLS_KEY", None),
                ("NESTRS_HTTP__TLS_CERT_FILE", None),
                ("NESTRS_HTTP__TLS_KEY_FILE", None),
            ],
            || {
                assert!(TlsConfig::from_env().expect("no error").is_none());
            },
        );
    }

    #[test]
    fn from_env_reads_inline_pem_pair() {
        with_env(
            &[
                ("NESTRS_HTTP__TLS_CERT", Some("--CERT--")),
                ("NESTRS_HTTP__TLS_KEY", Some("--KEY--")),
                ("NESTRS_HTTP__TLS_CERT_FILE", None),
                ("NESTRS_HTTP__TLS_KEY_FILE", None),
            ],
            || {
                let cfg = TlsConfig::from_env().expect("no error").expect("Some");
                assert_eq!(cfg.cert, b"--CERT--");
                assert_eq!(cfg.key, b"--KEY--");
            },
        );
    }

    #[test]
    fn from_env_fails_when_only_cert_is_set() {
        with_env(
            &[
                ("NESTRS_HTTP__TLS_CERT", Some("--CERT--")),
                ("NESTRS_HTTP__TLS_KEY", None),
                ("NESTRS_HTTP__TLS_CERT_FILE", None),
                ("NESTRS_HTTP__TLS_KEY_FILE", None),
            ],
            || {
                let err = TlsConfig::from_env().expect_err("half-config is rejected");
                let msg = err.to_string();
                assert!(msg.contains("KEY"), "must name the missing var: {msg}");
            },
        );
    }

    #[test]
    fn from_env_fails_when_only_key_is_set() {
        with_env(
            &[
                ("NESTRS_HTTP__TLS_CERT", None),
                ("NESTRS_HTTP__TLS_KEY", Some("--KEY--")),
                ("NESTRS_HTTP__TLS_CERT_FILE", None),
                ("NESTRS_HTTP__TLS_KEY_FILE", None),
            ],
            || {
                let err = TlsConfig::from_env().expect_err("half-config is rejected");
                let msg = err.to_string();
                assert!(msg.contains("CERT"), "must name the missing var: {msg}");
            },
        );
    }

    #[test]
    fn from_env_reads_file_variants_when_inline_unset() {
        // Write two temp files we'll point the *_FILE vars at.
        let dir = std::env::temp_dir().join(format!("nestrs-tls-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, b"file-cert-bytes").expect("write cert");
        std::fs::write(&key_path, b"file-key-bytes").expect("write key");

        with_env(
            &[
                ("NESTRS_HTTP__TLS_CERT", None),
                ("NESTRS_HTTP__TLS_KEY", None),
                (
                    "NESTRS_HTTP__TLS_CERT_FILE",
                    Some(cert_path.to_str().unwrap()),
                ),
                (
                    "NESTRS_HTTP__TLS_KEY_FILE",
                    Some(key_path.to_str().unwrap()),
                ),
            ],
            || {
                let cfg = TlsConfig::from_env().expect("no error").expect("Some");
                assert_eq!(cfg.cert, b"file-cert-bytes");
                assert_eq!(cfg.key, b"file-key-bytes");
            },
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
