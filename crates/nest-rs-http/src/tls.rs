use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use nest_rs_config::{Bound, ConfigService, DurationBounds, Floor, Setting};
use rustls::ServerConfig;
use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::crypto::{CryptoProvider, aws_lc_rs};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio_rustls::TlsAcceptor;

/// How often a file-sourced certificate is re-read; `0` turns watching off.
const DEFAULT_RELOAD_SECS: u64 = 60;

const RELOAD: DurationBounds = DurationBounds::secs(
    "TLS_RELOAD_SECS",
    "HttpTls::with_reload_secs",
    Floor::UnitsOrOff(Bound {
        count: 1,
        why: "the kernel's timers and the files' own write settle are not finer than that, and \
              a renewal is not that urgent",
    }),
    Bound {
        count: 24 * 60 * 60,
        why: "a certificate replaced on disk is served only once the next read finds it, and a \
              replacement after a key compromise is due within a day of it (CA/Browser Forum \
              Baseline Requirements §4.9.1.1) — a watch rarer than that serves the revoked one \
              past it",
    },
);

/// Where the PEM material came from: only files are reloaded.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TlsSource {
    Inline,
    Files { cert: PathBuf, key: PathBuf },
}

/// TLS material for the HTTP transport: a PEM certificate chain and private
/// key, handed to [`HttpTransport::tls`](crate::HttpTransport::tls).
///
/// ```
/// # use nest_rs_config::ConfigService;
/// # use nest_rs_http::{HttpTransport, HttpTls};
/// let env = ConfigService::for_namespace("http");
/// let http = HttpTransport::new().bind("0.0.0.0:3000");
/// let http = match HttpTls::from_env(&env, None)? {
///     Some(tls) => http.tls(tls),
///     None => http,
/// };
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Renewal without a restart
///
/// Material read from files (`<PREFIX>_HTTP__TLS_CERT_FILE` +
/// `<PREFIX>_HTTP__TLS_KEY_FILE`, or [`from_files`](Self::from_files)) is
/// **watched**: every [`reload_secs`](Self::with_reload_secs) — 60 by default,
/// `0` to disable — the pair is re-read, and a pair that has changed, *settled*
/// and can serve is swapped into the running listener without dropping a
/// connection. Anything short of that keeps the previous certificate serving,
/// with a `warn` on `nest_rs::http`. Inline PEM is loaded once.
#[derive(Clone)]
pub struct HttpTls {
    cert: Vec<u8>,
    key: Vec<u8>,
    source: TlsSource,
    reload_secs: u64,
}

/// Prints sizes only, so `HttpConfig`'s derived `Debug` cannot log the key.
impl std::fmt::Debug for HttpTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTls")
            .field("cert", &format_args!("<{} bytes>", self.cert.len()))
            .field("key", &format_args!("<{} bytes redacted>", self.key.len()))
            .field("source", &self.source)
            .field("reload_secs", &self.reload_secs)
            .finish()
    }
}

impl HttpTls {
    /// Build a TLS config from inline PEM certificate and private-key bytes,
    /// which are not watched — see [`from_files`](Self::from_files).
    pub fn new(cert: impl Into<Vec<u8>>, key: impl Into<Vec<u8>>) -> Self {
        Self {
            cert: cert.into(),
            key: key.into(),
            source: TlsSource::Inline,
            reload_secs: 0,
        }
    }

    /// Read PEM material from files and keep watching them, so a renewal that
    /// rewrites either path is picked up without a restart.
    pub fn from_files(cert: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Result<Self> {
        let cert_path = cert.into();
        let key_path = key.into();
        let cert = read_pem(&cert_path)?;
        let key = read_pem(&key_path)?;
        Ok(Self {
            cert,
            key,
            source: TlsSource::Files {
                cert: cert_path,
                key: key_path,
            },
            reload_secs: DEFAULT_RELOAD_SECS,
        })
    }

    /// How often a file-sourced pair is re-read, in seconds, from 1 to 86400 (a
    /// day); `0` disables watching. Ignored by inline material, which has no
    /// source to watch. A value outside the range fails the boot naming
    /// `<PREFIX>_HTTP__TLS_RELOAD_SECS`, as the variable's would.
    pub fn with_reload_secs(mut self, secs: u64) -> Self {
        self.reload_secs = secs;
        self
    }

    /// Read TLS material from `<PREFIX>_HTTP__TLS_CERT` / `<PREFIX>_HTTP__TLS_KEY`
    /// (PEM inline) or their `_FILE` variants (path the transport loads), read
    /// through [`ConfigService::material`]. `base` is what the field keeps when
    /// the environment configures neither half.
    ///
    /// `Ok(None)` when neither is present and `base` is `None` (serve plain
    /// HTTP). Fails if exactly one of the pair is configured.
    pub fn from_env(env: &ConfigService, base: Option<Self>) -> Result<Option<Self>> {
        let cert = env.material("TLS_CERT")?;
        let key = env.material("TLS_KEY")?;
        // The base the variable overlays: the default for a pair read here,
        // else the base's own, where `0` is off.
        let overlaid = match (&cert, &key, &base) {
            (Some(_), Some(_), _) => Some(Duration::from_secs(DEFAULT_RELOAD_SECS)),
            (_, _, Some(base)) => {
                (base.reload_secs > 0).then(|| Duration::from_secs(base.reload_secs))
            }
            _ => None,
        };
        let reload = RELOAD
            .read_optional(env, overlaid)?
            .map_or(0, |read| read.value.as_secs());
        match (cert, key) {
            (Some(Setting { value: cert, .. }), Some(Setting { value: key, .. })) => {
                // A mixed pair is not watched: reloading one half would pair a
                // certificate with a key it no longer matches.
                let config = match (cert.path, key.path) {
                    (Some(cert_path), Some(key_path)) => Self {
                        cert: cert.bytes,
                        key: key.bytes,
                        source: TlsSource::Files {
                            cert: cert_path,
                            key: key_path,
                        },
                        reload_secs: reload,
                    },
                    _ => Self::new(cert.bytes, key.bytes),
                };
                Ok(Some(config))
            }
            (None, None) => Ok(base.map(|base| base.with_reload_secs(reload))),
            (Some(cert), None) => Err(half_pair(env, &cert, "TLS_KEY", base.is_some())),
            (None, Some(key)) => Err(half_pair(env, &key, "TLS_CERT", base.is_some())),
        }
    }

    /// The listener's side of the material: the handshake every accepted
    /// socket runs, and the watch that renews its certificate when the pair
    /// came from files.
    ///
    /// The pair is refused here when it cannot serve, before the port is bound.
    pub(crate) fn into_listener(self) -> Result<ListenerTls> {
        let current = certified_key(&self.cert, &self.key)?;
        // A pair built in code reaches here without a config read.
        if self.reload_secs > 0 {
            RELOAD.check(
                <crate::HttpConfig as nest_rs_config::Namespaced>::NAMESPACE,
                RELOAD.field(),
                Duration::from_secs(self.reload_secs),
            )?;
        }
        let resolver = Arc::new(ReloadingResolver {
            current: RwLock::new(Arc::new(current)),
        });
        let mut config = ServerConfig::builder_with_provider(crypto_provider())
            .with_safe_default_protocol_versions()
            .context("the crypto provider speaks no TLS version rustls serves")?
            .with_no_client_auth()
            .with_cert_resolver(Arc::clone(&resolver) as Arc<dyn ResolvesServerCert>);
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let renewal = match self.source {
            TlsSource::Files { cert, key } if self.reload_secs > 0 => {
                let watcher = Watcher {
                    cert_path: cert,
                    key_path: key,
                    cert: self.cert,
                    key: self.key,
                    interval: Duration::from_secs(self.reload_secs),
                };
                Some(Box::pin(watcher.renew(resolver)) as Renewal)
            }
            _ => None,
        };
        Ok(ListenerTls {
            acceptor: TlsAcceptor::from(Arc::new(config)),
            renewal,
        })
    }
}

/// The watch that swaps a renewed certificate into the listener; it never ends.
pub(crate) type Renewal = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The TLS half of the listener: the handshake, and the renewal to run beside it.
pub(crate) struct ListenerTls {
    pub(crate) acceptor: TlsAcceptor,
    pub(crate) renewal: Option<Renewal>,
}

/// The certificate every handshake presents, replaced whole by a renewal, so a
/// connection already open keeps the session it handshook.
#[derive(Debug)]
struct ReloadingResolver {
    current: RwLock<Arc<CertifiedKey>>,
}

impl ReloadingResolver {
    fn replace(&self, renewed: CertifiedKey) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(renewed);
    }
}

impl ResolvesServerCert for ReloadingResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(
            &self.current.read().unwrap_or_else(PoisonError::into_inner),
        ))
    }
}

/// The process-wide crypto provider the listener builds with, aws-lc-rs
/// installed first when the app has not chosen one: a client built later from
/// the default — `redis`'s — then finds one, rather than panicking when two
/// providers are compiled in.
fn crypto_provider() -> Arc<CryptoProvider> {
    if let Some(installed) = CryptoProvider::get_default() {
        return Arc::clone(installed);
    }
    // Losing a race to another installer leaves theirs in place, and theirs is
    // the one handed back.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "losing the race leaves the other installer's provider, which is read back below"
    )]
    let _ = aws_lc_rs::default_provider().install_default();
    CryptoProvider::get_default()
        .map_or_else(|| Arc::new(aws_lc_rs::default_provider()), Arc::clone)
}

/// The one place a certificate is built from a PEM pair, at boot and on
/// renewal, refusing what cannot serve: a pair that does not parse, an empty
/// chain, and a pair whose halves do not correspond, which would install and
/// fail every handshake.
///
/// A parse failure is described, never quoted: the parser's error carries the
/// line it choked on, which may hold the whole private key.
#[expect(
    clippy::map_err_ignore,
    reason = "the PEM parser's error quotes the line it choked on, which may hold the private key"
)]
fn certified_key(cert: &[u8], key: &[u8]) -> Result<CertifiedKey> {
    let chain = CertificateDer::pem_slice_iter(cert)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| anyhow::anyhow!("the certificate is not valid PEM: {PEM_SHAPE}"))?;
    anyhow::ensure!(
        !chain.is_empty(),
        "the certificate holds no CERTIFICATE block — an empty or truncated file reads as a \
         chain with nothing in it, which every handshake then fails to present",
    );
    let key = PrivateKeyDer::from_pem_slice(key).map_err(|error| match error {
        rustls::pki_types::pem::Error::NoItemsFound => anyhow::anyhow!(
            "the private key holds no PRIVATE KEY block — an empty file, a certificate or a \
             public key in its place, or an encrypted key, which has to be decrypted first"
        ),
        _ => anyhow::anyhow!("the private key is not valid PEM: {PEM_SHAPE}"),
    })?;
    // Loaded with aws-lc-rs whatever provider the app installed: its keys
    // expose their public half, which `keys_match` needs.
    let signing = any_supported_type(&key)
        .context("the private key is not a type this build of rustls can sign with")?;
    let certified = CertifiedKey::new(chain, signing);
    certified.keys_match().map_err(|error| match error {
        rustls::Error::InconsistentKeys(_) => anyhow::anyhow!(
            "the certificate and the private key do not correspond — the chain has to start \
             with this key's own certificate, and a renewal that writes the two as separate \
             operations is observable half-done; installing that pair fails every handshake"
        ),
        other => anyhow::anyhow!("the certificate cannot be read as X.509: {other}"),
    })?;
    Ok(certified)
}

/// What a PEM value has to look like, said instead of quoting one that does not.
const PEM_SHAPE: &str = "each -----BEGIN and -----END line stands on a line of its own, with \
     intact base64 between them, and a value whose line breaks were collapsed or escaped does \
     not parse";

/// Polls the PEM pair and reports a renewal. Polling, not an OS watch: an atomic
/// replace swaps the inode out from under a watch.
struct Watcher {
    cert_path: PathBuf,
    key_path: PathBuf,
    cert: Vec<u8>,
    key: Vec<u8>,
    interval: Duration,
}

impl Watcher {
    /// Wait until the pair on disk differs from the pair in hand and reads back
    /// identical twice, so a pair written in two operations is never caught
    /// half-done.
    async fn next_settled(&mut self) -> (Vec<u8>, Vec<u8>) {
        let mut pending: Option<(Vec<u8>, Vec<u8>)> = None;
        loop {
            tokio::time::sleep(self.interval).await;
            let (Some(cert), Some(key)) = self.read_pair().await else {
                pending = None;
                continue;
            };
            if cert == self.cert && key == self.key {
                pending = None;
                continue;
            }
            let current = (cert, key);
            if pending.as_ref() != Some(&current) {
                // First sighting, or it moved again — wait one more tick.
                pending = Some(current);
                continue;
            }
            // Held as the pair in hand even if it cannot serve, so a bad pair is
            // reported once rather than every interval.
            let (cert, key) = current;
            self.cert = cert.clone();
            self.key = key.clone();
            return (cert, key);
        }
    }

    /// Swap every renewal into `resolver`, for as long as the listener runs.
    async fn renew(mut self, resolver: Arc<ReloadingResolver>) {
        loop {
            resolver.replace(self.next_renewal().await);
        }
    }

    /// The next settled pair that can serve ([`certified_key`]); one that
    /// cannot is refused with a `warn` and the working certificate keeps serving.
    async fn next_renewal(&mut self) -> CertifiedKey {
        loop {
            let (cert, key) = self.next_settled().await;
            let renewed = match certified_key(&cert, &key) {
                Ok(renewed) => renewed,
                Err(error) => {
                    tracing::warn!(
                        target: crate::target::HTTP,
                        cert = %self.cert_path.display(),
                        key = %self.key_path.display(),
                        error = format!("{error:#}"),
                        "renewed tls material was refused; keeping the certificate in use",
                    );
                    continue;
                }
            };
            tracing::info!(
                target: crate::target::HTTP,
                cert = %self.cert_path.display(),
                key = %self.key_path.display(),
                "tls certificate renewed on disk",
            );
            return renewed;
        }
    }

    /// Both halves, read off the async task: a file read blocks its thread.
    async fn read_pair(&self) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        let (cert, key) = (self.cert_path.clone(), self.key_path.clone());
        match tokio::task::spawn_blocking(move || (Self::read(&cert), Self::read(&key))).await {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!(
                    target: crate::target::HTTP,
                    cert = %self.cert_path.display(),
                    key = %self.key_path.display(),
                    error = %nest_rs_core::error_message(&error),
                    "tls material could not be re-read; keeping the certificate in use",
                );
                (None, None)
            }
        }
    }

    /// A read failure is reported and skipped, never fatal: a renewal tool that
    /// unlinks before it writes would otherwise take the listener down.
    fn read(path: &Path) -> Option<Vec<u8>> {
        // Refuses a FIFO, which would stall the watcher.
        match nest_rs_config::read_material(path) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                tracing::warn!(
                    target: crate::target::HTTP,
                    path = %path.display(),
                    error = %nest_rs_core::error_message(&error),
                    "tls material could not be re-read; keeping the certificate in use",
                );
                None
            }
        }
    }
}

/// The refusal for half a pair, naming the half as the deployment spelled it and
/// both spellings of the other; over a pinned pair, that half is only not replaced.
fn half_pair(
    env: &ConfigService,
    set: &Setting<nest_rs_config::Material>,
    other_key: &str,
    pinned: bool,
) -> anyhow::Error {
    let set_var = set.var();
    let other = env.spellings(other_key);
    if pinned {
        anyhow::anyhow!(
            "{set_var} is set, but the other half of the pair is the one pinned in code — set \
             {other} beside it, so the pair is replaced whole rather than mixed"
        )
    } else {
        anyhow::anyhow!(
            "{set_var} is set without the other half of the pair — set {other} beside it, or \
             neither to serve plain HTTP"
        )
    }
}

fn read_pem(path: &Path) -> Result<Vec<u8>> {
    nest_rs_config::read_material(path)
        .with_context(|| format!("reading TLS material at {}", path.display()))
}

#[cfg(test)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "the tests tear down temp files best-effort, and a watchdog's receiver may be gone"
)]
mod tests {
    use std::sync::LazyLock;

    use nest_rs_testing::{TestAuthority, TestCertificate};

    use super::*;

    /// Two pairs under one authority, issued for this test process.
    static PAIRS: LazyLock<(TestCertificate, TestCertificate)> = LazyLock::new(|| {
        let authority = TestAuthority::new();
        (
            authority.server(&["a.nestrs.test"]),
            authority.server(&["b.nestrs.test"]),
        )
    });

    fn cert_a() -> &'static [u8] {
        PAIRS.0.cert.as_bytes()
    }

    fn key_a() -> &'static [u8] {
        PAIRS.0.key.as_bytes()
    }

    fn key_b() -> &'static [u8] {
        PAIRS.1.key.as_bytes()
    }

    /// A FIFO in a scratch directory, for the two readers that must not block
    /// on one.
    #[cfg(unix)]
    fn fifo(label: &str) -> (ScratchDir, std::path::PathBuf) {
        let dir = ScratchDir(
            std::env::temp_dir().join(format!("nest-rs-http-{label}-{}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(&*dir);
        std::fs::create_dir_all(&*dir).expect("a scratch directory");
        let fifo = dir.join("pipe.pem");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "a FIFO to point at");
        (dir, fifo)
    }

    /// `from_files` naming a FIFO fails the boot instead of blocking on the
    /// pipe forever.
    #[cfg(unix)]
    #[test]
    fn from_files_refuses_a_fifo_without_blocking() {
        let (_dir, fifo) = fifo("from-files");
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(HttpTls::from_files(path.clone(), path).is_err());
        });
        let refused = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("returns instead of blocking on the pipe");
        assert!(refused, "a FIFO is not TLS material");
    }

    /// The renewal watcher re-reading a path that became a FIFO skips it and
    /// keeps the certificate in use, instead of stalling renewal for good.
    #[cfg(unix)]
    #[test]
    fn the_watcher_skips_a_fifo_without_blocking() {
        let (_dir, fifo) = fifo("watcher");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Watcher::read(&fifo).is_none());
        });
        let skipped = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("returns instead of blocking on the pipe");
        assert!(skipped, "a FIFO is not re-read as material");
    }

    fn watcher(dir: &std::path::Path) -> Watcher {
        Watcher {
            cert_path: dir.join("tls.pem"),
            key_path: dir.join("tls.key.pem"),
            cert: b"--IN-USE-CERT--".to_vec(),
            key: b"--IN-USE-KEY--".to_vec(),
            interval: Duration::from_millis(5),
        }
    }

    /// A directory the test owns, removed on drop, so a failing assertion leaks none.
    struct ScratchDir(std::path::PathBuf);

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for ScratchDir {
        type Target = std::path::Path;

        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    fn scratch_dir(tag: &str) -> ScratchDir {
        let dir = std::env::temp_dir().join(format!("nest_rs_tls_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        ScratchDir(dir)
    }

    #[test]
    fn material_that_cannot_be_re_read_is_reported_and_the_listener_left_alone() {
        let logs = nest_rs_testing::LogCapture::install();
        let dir = scratch_dir("unreadable");
        let watcher = watcher(&dir);

        assert!(
            Watcher::read(&watcher.cert_path).is_none(),
            "a path with nothing at it reads as nothing, not as empty material",
        );

        let event = logs.expect_one(
            "nest_rs::http",
            "tls material could not be re-read; keeping the certificate in use",
        );
        assert_eq!(event.level, "warn");
        assert!(
            event.field("path").is_some_and(|p| p.ends_with("tls.pem")),
            "the event names which half of the pair, got {:?}",
            event.fields,
        );
        assert!(
            event.field("error").is_some(),
            "…and why, got {:?}",
            event.fields,
        );
    }

    #[tokio::test]
    async fn a_renewal_that_could_not_serve_is_refused_rather_than_published() {
        let logs = nest_rs_testing::LogCapture::install();
        let dir = scratch_dir("refused");
        let mut watcher = watcher(&dir);
        std::fs::write(&watcher.cert_path, b"not a certificate").expect("write the cert half");
        std::fs::write(&watcher.key_path, b"not a key").expect("write the key half");

        // `next_renewal` loops until something serves, so it must time out.
        let published = tokio::time::timeout(Duration::from_millis(300), watcher.next_renewal())
            .await
            .ok();
        assert!(
            published.is_none(),
            "material that cannot serve is never published",
        );

        let refusals = logs.find(
            "nest_rs::http",
            "renewed tls material was refused; keeping the certificate in use",
        );
        let event = refusals
            .first()
            .unwrap_or_else(|| panic!("the refusal is reported: {:#?}", logs.events()));
        assert_eq!(event.level, "warn");
        assert!(
            event.field("cert").is_some() && event.field("key").is_some(),
            "the event names both paths, since either half can be the bad one, got {:?}",
            event.fields,
        );
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("certificate")),
            "…and which check failed, got {:?}",
            event.fields,
        );
        assert!(
            logs.find("nest_rs::http", "tls certificate renewed on disk")
                .is_empty(),
            "and nothing announced a renewal that did not happen: {:#?}",
            logs.events(),
        );
    }

    #[test]
    fn new_round_trips_bytes() {
        let cfg = HttpTls::new(b"--CERT--".to_vec(), b"--KEY--".to_vec());
        assert_eq!(cfg.cert, b"--CERT--");
        assert_eq!(cfg.key, b"--KEY--");
    }

    #[test]
    fn debug_redacts_key_bytes() {
        let cfg = HttpTls::new(vec![0; 128], b"super secret key material".to_vec());
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("super secret"), "key leaked: {debug}");
        assert!(
            debug.contains("redacted"),
            "missing redaction marker: {debug}"
        );
        assert!(debug.contains("128 bytes"), "cert length missing: {debug}");
    }

    fn tls_env<'a>(vars: impl IntoIterator<Item = (&'a str, &'a str)>) -> ConfigService {
        ConfigService::with_vars("http", vars)
    }

    #[test]
    fn from_env_is_none_when_no_tls_vars_are_set() {
        assert!(
            HttpTls::from_env(&tls_env([]), None)
                .expect("no error")
                .is_none()
        );
    }

    #[test]
    fn from_env_keeps_the_base_when_no_tls_vars_are_set() {
        let base = HttpTls::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        let kept = HttpTls::from_env(&tls_env([]), Some(base))
            .expect("no error")
            .expect("the base survives an env that configures no TLS");
        assert_eq!(kept.cert, b"--PINNED-CERT--");
    }

    #[test]
    fn from_env_overrides_the_base_when_both_halves_are_set() {
        let base = HttpTls::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        let cfg = HttpTls::from_env(
            &tls_env([("TLS_CERT", "--ENV-CERT--"), ("TLS_KEY", "--ENV-KEY--")]),
            Some(base),
        )
        .expect("no error")
        .expect("Some");
        assert_eq!(cfg.cert, b"--ENV-CERT--");
        assert_eq!(cfg.key, b"--ENV-KEY--");
    }

    /// Half a pair is named as the deployment spelled it: a certificate given
    /// by path is `TLS_CERT_FILE` in the refusal, not the inline name nobody set.
    #[test]
    fn a_half_pair_names_the_spelling_the_deployment_used() {
        let file =
            std::env::temp_dir().join(format!("nestrs-http-half-{}.pem", std::process::id()));
        std::fs::write(&file, cert_a()).expect("write the certificate");
        let cert = file.to_str().expect("a UTF-8 temporary path");
        let err = HttpTls::from_env(&tls_env([("TLS_CERT_FILE", cert)]), None)
            .expect_err("half a pair is refused");
        let msg = err.to_string();
        assert!(
            msg.starts_with(&nest_rs_config::var_name("http", "TLS_CERT_FILE")),
            "{msg}"
        );
    }

    /// Over a pair pinned in code, a deployment half is still refused — the
    /// pair would be mixed — but the pin holds the other half, so the sentence
    /// must not say there is none.
    #[test]
    fn a_half_pair_over_a_pinned_pair_does_not_say_the_other_half_is_missing() {
        let base = HttpTls::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        let err = HttpTls::from_env(&tls_env([("TLS_KEY", "--ENV-KEY--")]), Some(base))
            .expect_err("a mixed pair is refused");
        let msg = err.to_string();
        assert!(msg.contains("pinned in code"), "{msg}");
        assert!(!msg.contains("without"), "{msg}");
    }

    #[test]
    fn from_env_reads_inline_pem_pair() {
        let cfg = HttpTls::from_env(
            &tls_env([("TLS_CERT", "--CERT--"), ("TLS_KEY", "--KEY--")]),
            None,
        )
        .expect("no error")
        .expect("Some");
        assert_eq!(cfg.cert, b"--CERT--");
        assert_eq!(cfg.key, b"--KEY--");
    }

    #[test]
    fn from_env_fails_when_only_cert_is_set() {
        let err = HttpTls::from_env(&tls_env([("TLS_CERT", "--CERT--")]), None)
            .expect_err("half-config is rejected");
        let msg = err.to_string();
        assert!(msg.contains("KEY"), "must name the missing var: {msg}");
    }

    #[test]
    fn from_env_fails_when_only_key_is_set() {
        let err = HttpTls::from_env(&tls_env([("TLS_KEY", "--KEY--")]), None)
            .expect_err("half-config is rejected");
        let msg = err.to_string();
        assert!(msg.contains("CERT"), "must name the missing var: {msg}");
    }

    #[test]
    fn from_env_fails_on_a_half_config_even_over_a_complete_base() {
        let base = HttpTls::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        assert!(HttpTls::from_env(&tls_env([("TLS_CERT", "--ENV-CERT--")]), Some(base),).is_err());
    }

    #[test]
    fn inline_material_is_not_watchable() {
        let cfg = HttpTls::new(b"--CERT--".to_vec(), b"--KEY--".to_vec());
        assert_eq!(cfg.source, TlsSource::Inline);
        assert_eq!(cfg.reload_secs, 0);
    }

    #[test]
    fn from_env_rejects_an_unparseable_reload_interval() {
        let err = HttpTls::from_env(
            &tls_env([
                ("TLS_CERT", "--CERT--"),
                ("TLS_KEY", "--KEY--"),
                ("TLS_RELOAD_SECS", "hourly"),
            ]),
            None,
        )
        .expect_err("a typo aborts the boot rather than silently disabling the watch");
        assert!(
            err.to_string().contains("RELOAD_SECS"),
            "must name the variable: {err}",
        );
    }

    #[test]
    fn from_env_applies_the_reload_interval_to_a_pinned_base() {
        let base = HttpTls::new(b"--CERT--".to_vec(), b"--KEY--".to_vec());
        let cfg = HttpTls::from_env(&tls_env([("TLS_RELOAD_SECS", "5")]), Some(base))
            .expect("no error")
            .expect("Some");
        assert_eq!(cfg.reload_secs, 5);
    }

    #[test]
    fn a_reload_interval_past_a_day_is_refused_from_either_side() {
        let var = nest_rs_config::var_name("http", "TLS_RELOAD_SECS");
        let from_env = HttpTls::from_env(
            &tls_env([
                ("TLS_CERT", "--CERT--"),
                ("TLS_KEY", "--KEY--"),
                ("TLS_RELOAD_SECS", "86401"),
            ]),
            None,
        )
        .expect_err("past a day")
        .to_string();
        assert!(
            from_env.contains(&var) && from_env.contains("must be at most 86400 seconds"),
            "{from_env}"
        );
        let base =
            HttpTls::new(b"--CERT--".to_vec(), b"--KEY--".to_vec()).with_reload_secs(u64::MAX);
        let pinned = HttpTls::from_env(&tls_env([]), Some(base))
            .expect_err("a pinned interval past a day")
            .to_string();
        assert!(
            pinned.contains(&var) && pinned.contains("`HttpTls::with_reload_secs` set in code"),
            "{pinned}"
        );
        let off = HttpTls::new(b"--CERT--".to_vec(), b"--KEY--".to_vec()).with_reload_secs(60);
        let cfg = HttpTls::from_env(&tls_env([("TLS_RELOAD_SECS", "0")]), Some(off))
            .expect("`0` is off")
            .expect("Some");
        assert_eq!(cfg.reload_secs, 0);
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn from_env_file_variants_are_watched_by_default() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("cert.pem", "file-cert-bytes")?;
            jail.create_file("key.pem", "file-key-bytes")?;
            let cfg = HttpTls::from_env(
                &tls_env([("TLS_CERT_FILE", "cert.pem"), ("TLS_KEY_FILE", "key.pem")]),
                None,
            )
            .expect("no error")
            .expect("Some");
            assert!(matches!(cfg.source, TlsSource::Files { .. }));
            assert_eq!(cfg.reload_secs, DEFAULT_RELOAD_SECS);

            let mixed = HttpTls::from_env(
                &tls_env([("TLS_CERT", "--INLINE-CERT--"), ("TLS_KEY_FILE", "key.pem")]),
                None,
            )
            .expect("no error")
            .expect("Some");
            assert_eq!(mixed.source, TlsSource::Inline);
            assert_eq!(mixed.reload_secs, 0);
            Ok(())
        });
    }

    #[tokio::test]
    async fn a_pair_that_is_still_changing_is_never_installed() {
        let dir = scratch_dir("settle");
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, "cert-v1").expect("write cert");
        std::fs::write(&key_path, "key-v1").expect("write key");

        let mut watcher = Watcher {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            cert: b"cert-v1".to_vec(),
            key: b"key-v1".to_vec(),
            interval: Duration::from_millis(5),
        };

        let writer = tokio::spawn({
            let cert_path = cert_path.clone();
            let key_path = key_path.clone();
            async move {
                for step in 2..8 {
                    std::fs::write(&cert_path, format!("cert-v{step}")).expect("write cert");
                    std::fs::write(&key_path, format!("key-v{step}")).expect("write key");
                    tokio::time::sleep(Duration::from_millis(6)).await;
                }
            }
        });

        let installed = watcher.next_settled().await;
        writer.await.expect("writer task");

        let cert = String::from_utf8(installed.0).expect("utf8 cert");
        let key = String::from_utf8(installed.1).expect("utf8 key");
        assert_eq!(
            cert.trim_start_matches("cert-"),
            key.trim_start_matches("key-"),
            "installed a certificate and a key from different writes: {cert} / {key}",
        );
    }

    #[tokio::test]
    async fn a_watcher_reports_a_renewal_and_skips_an_unreadable_read() {
        let dir = scratch_dir("watch");
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        // The certificate starts absent, as when a renewal tool unlinks first.
        std::fs::write(&key_path, "key-v1").expect("write key");

        let mut watcher = Watcher {
            cert_path: cert_path.clone(),
            key_path: key_path.clone(),
            cert: b"cert-v1".to_vec(),
            key: b"key-v1".to_vec(),
            interval: Duration::from_millis(5),
        };

        let writer = {
            let cert_path = cert_path.clone();
            let key_path = key_path.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                std::fs::write(&cert_path, "cert-v2").expect("write cert");
                std::fs::write(&key_path, "key-v2").expect("rewrite key");
            })
        };

        watcher.next_settled().await;
        writer.await.expect("writer task");
        assert_eq!(watcher.cert, b"cert-v2");
        assert_eq!(watcher.key, b"key-v2");
    }

    /// A key whose line breaks were collapsed is one line the parser's error
    /// would quote whole; the refusal quotes nothing, not even as bytes.
    #[test]
    fn a_pair_that_does_not_parse_is_described_and_never_quoted() {
        let cert_a = cert_a();
        let key_a = std::str::from_utf8(key_a()).expect("PEM is text");
        let secret = key_a.lines().nth(1).expect("a base64 line of the key");
        let as_bytes = secret.as_bytes()[..4]
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for collapsed in [key_a.replace('\n', " "), key_a.replace('\n', "\\n")] {
            let refused = certified_key(cert_a, collapsed.as_bytes())
                .expect_err("a key on one line does not parse");
            for shown in [format!("{refused:#}"), format!("{refused:?}")] {
                assert!(
                    shown.contains("not valid PEM"),
                    "names what is wrong: {shown}"
                );
                assert!(
                    !shown.contains(&secret[..12]),
                    "never quotes the key: {shown}"
                );
                assert!(!shown.contains(&as_bytes), "not even as bytes: {shown}");
            }
        }
    }

    /// The listener loads its key with aws-lc-rs, so a provider whose keys
    /// hide their public half cannot let a mismatched pair through the check.
    #[test]
    fn a_mismatched_pair_is_refused_whatever_provider_the_app_installed() {
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
        rustls::crypto::CryptoProvider {
            key_provider: &OpaqueKeys,
            ..rustls::crypto::aws_lc_rs::default_provider()
        }
        .install_default()
        .expect("each test runs in a process of its own, with no provider installed yet");

        let cert_a = cert_a();
        let key_b = key_b();
        let refused = certified_key(cert_a, key_b).expect_err("a mismatched pair is refused");
        assert!(
            format!("{refused:#}").contains("do not correspond"),
            "{refused:#}"
        );
    }

    /// And a key the installed provider cannot load at all is still one the
    /// listener serves, so the check does not refuse it either.
    #[test]
    fn a_pair_the_listener_serves_is_accepted_whatever_provider_the_app_installed() {
        #[derive(Debug)]
        struct NoKeys;
        impl rustls::crypto::KeyProvider for NoKeys {
            fn load_private_key(
                &self,
                _key: PrivateKeyDer<'static>,
            ) -> std::result::Result<Arc<dyn rustls::sign::SigningKey>, rustls::Error> {
                Err(rustls::Error::General(
                    "this provider loads no key".to_owned(),
                ))
            }
        }
        rustls::crypto::CryptoProvider {
            key_provider: &NoKeys,
            ..rustls::crypto::aws_lc_rs::default_provider()
        }
        .install_default()
        .expect("each test runs in a process of its own, with no provider installed yet");

        let cert_a = cert_a();
        let key_a = key_a();
        certified_key(cert_a, key_a).expect("a pair the listener serves is not refused");
    }

    /// A key file holding no private key — empty, or a certificate in its
    /// place — says so, rather than blaming line breaks it does not have.
    #[test]
    fn a_key_file_holding_no_private_key_says_so() {
        let cert_a = cert_a();
        for (label, key) in [("empty", &b""[..]), ("a certificate", cert_a)] {
            let refused =
                certified_key(cert_a, key).expect_err("a file with no key in it is refused");
            assert!(
                format!("{refused:#}").contains("no PRIVATE KEY block"),
                "{label}: {refused:#}"
            );
        }
    }

    #[test]
    fn a_pair_that_cannot_serve_is_refused_before_it_is_published() {
        let cert_a = cert_a();
        let key_a = key_a();
        let key_b = key_b();

        certified_key(cert_a, key_a).expect("the fixture pair corresponds");

        let mismatched = certified_key(cert_a, key_b).expect_err("a mismatched pair is refused");
        assert!(
            format!("{mismatched:#}").contains("do not correspond"),
            "the refusal names what is wrong: {mismatched:#}",
        );

        let empty = certified_key(b"", key_a).expect_err("an empty certificate is refused");
        assert!(
            format!("{empty:#}").contains("no CERTIFICATE block"),
            "the refusal names what is wrong: {empty:#}",
        );

        assert!(
            certified_key(b"-----BEGIN CERTIFICATE-----\nnot base64\n", key_a).is_err(),
            "and material that does not parse is still refused",
        );
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn from_env_reads_file_variants_when_inline_unset() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("cert.pem", "file-cert-bytes")?;
            jail.create_file("key.pem", "file-key-bytes")?;
            let cfg = HttpTls::from_env(
                &tls_env([("TLS_CERT_FILE", "cert.pem"), ("TLS_KEY_FILE", "key.pem")]),
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
