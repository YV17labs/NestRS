use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::{self, BoxStream, StreamExt};
use nest_rs_config::{Bound, ConfigService, DurationBounds, Floor, Setting};
use poem::listener::{RustlsCertificate, RustlsConfig};
use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;

/// How often a file-sourced certificate is re-read. A renewal lands within one
/// interval; `0` turns watching off.
const DEFAULT_RELOAD_SECS: u64 = 60;

/// The watch interval's range, the variable that sets it, and why.
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

/// Where the PEM material came from — which is what decides whether it can be
/// reloaded. Inline bytes are the deployment's final word; files are a *source*
/// a renewal rewrites underneath the running process.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TlsSource {
    Inline,
    Files { cert: PathBuf, key: PathBuf },
}

/// TLS material for the HTTP transport: a PEM certificate chain and private
/// key, handed to [`HttpTransport::tls`](crate::HttpTransport::tls).
///
/// ```no_run
/// # use nest_rs_config::ConfigService;
/// # use nest_rs_http::{HttpTransport, HttpTls};
/// let env = ConfigService::for_namespace("http");
/// let mut http = HttpTransport::new().bind("0.0.0.0:3000");
/// if let Some(tls) = HttpTls::from_env(&env, None)? {
///     http = http.tls(tls);
/// }
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// # Renewal without a restart
///
/// Material read from files (`NESTRS_HTTP__TLS_CERT_FILE` +
/// `NESTRS_HTTP__TLS_KEY_FILE`, or [`from_files`](Self::from_files)) is
/// **watched**: every [`reload_secs`](Self::with_reload_secs) — 60 by default,
/// `0` to disable — the pair is re-read, and a pair that has changed, *settled*
/// and can actually serve is swapped into the running `rustls` config. The
/// listener is never rebuilt, so no connection is dropped and no port is
/// released.
///
/// Anything short of that leaves the previous certificate serving and says so
/// with a `warn` on `nest_rs::http`: a read that fails, material that does not
/// parse, an empty certificate file, and a certificate whose key does not
/// correspond to it. The last two are the ones a settled read cannot see —
/// they parse, and they fail every handshake.
///
/// Inline PEM has no source to watch, so it is loaded once as before.
#[derive(Clone)]
pub struct HttpTls {
    cert: Vec<u8>,
    key: Vec<u8>,
    source: TlsSource,
    reload_secs: u64,
}

/// Manual `Debug` so `HttpConfig`'s derived `Debug` cannot leak the private
/// key to a log line — only sizes are printed.
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
    /// Build a TLS config from PEM certificate and private-key bytes. Inline
    /// material has no source to watch — see [`from_files`](Self::from_files)
    /// for the reloadable form.
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
    /// `NESTRS_HTTP__TLS_RELOAD_SECS`, as the variable's would.
    pub fn with_reload_secs(mut self, secs: u64) -> Self {
        self.reload_secs = secs;
        self
    }

    /// Read TLS material from `NESTRS_HTTP__TLS_CERT` / `NESTRS_HTTP__TLS_KEY`
    /// (PEM inline) or their `_FILE` variants (path the transport loads), read
    /// through [`ConfigService::material`] — which refuses a half set both ways.
    /// `base` is what the field keeps when the environment configures neither
    /// half.
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
        let cert = env.material("TLS_CERT")?;
        let key = env.material("TLS_KEY")?;
        // The interval the setting overlays: the default for a pair read from
        // files here, and otherwise the base's own, where `0` is off.
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
                // Both halves read from files ⇒ a watchable source. A mixed
                // pair (one inline, one from a file) is not: reloading half a
                // pair would swap in a certificate its key no longer matches.
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

    /// The stream poem's `rustls` listener consumes: the material as loaded,
    /// then one item per observed renewal. The listener swaps the acceptor in
    /// place, so nothing about the socket changes.
    ///
    /// Inline material yields exactly once and the stream then idles — poem
    /// keeps serving the last config it saw.
    ///
    /// The initial pair is checked here, and that is not belt-and-braces: poem
    /// validates a `RustlsConfig` handed to it *directly*, but a **stream** of
    /// them takes the blanket `IntoTlsConfigStream` impl, whose `into_stream` is
    /// `Ok(self)`. Without this the process would boot, bind and accept TCP with
    /// unusable material, then drop every connection — a check that ran before
    /// this type became a stream, restored at the seam that removed it.
    pub(crate) fn into_rustls_stream(self) -> Result<BoxStream<'static, RustlsConfig>> {
        validate_pair(&self.cert, &self.key)?;
        // A pair built in code reaches the listener without a config read, so
        // its interval is held to the variable's range here, where it is spent.
        if self.reload_secs > 0 {
            RELOAD.check(
                <crate::HttpConfig as nest_rs_config::Namespaced>::NAMESPACE,
                RELOAD.field(),
                Duration::from_secs(self.reload_secs),
            )?;
        }
        let HttpTls {
            cert,
            key,
            source,
            reload_secs,
        } = self;
        // Built from clones because the buffers are handed to the `Watcher`
        // below as the pair in hand; inline material has no watcher and drops
        // them right after.
        let initial = rustls_config(cert.clone(), key.clone());
        let head = stream::once(async move { initial });
        let TlsSource::Files {
            cert: cert_path,
            key: key_path,
        } = source
        else {
            return Ok(head.boxed());
        };
        if reload_secs == 0 {
            return Ok(head.boxed());
        }
        let watcher = Watcher {
            cert_path,
            key_path,
            cert,
            key,
            interval: Duration::from_secs(reload_secs),
        };
        Ok(head
            .chain(stream::unfold(watcher, |mut watcher| async move {
                let next = watcher.next_renewal().await;
                Some((next, watcher))
            }))
            .boxed())
    }
}

/// The one place a `RustlsConfig` is built from a PEM pair — the boot's initial
/// material and every accepted renewal, so the two cannot be assembled
/// differently.
fn rustls_config(cert: Vec<u8>, key: Vec<u8>) -> RustlsConfig {
    RustlsConfig::new().fallback(RustlsCertificate::new().cert(cert).key(key))
}

/// Everything poem's parser would refuse, plus the two failures it structurally
/// cannot see: a chain with no certificate in it, and a pair whose halves do not
/// correspond. Both install cleanly through `with_cert_resolver` and then fail
/// **every** handshake — a total outage from material the process reported as
/// renewed.
///
/// This parses the same bytes poem is about to parse, which is a duplication
/// worth naming. It is safe in one direction only, and that is the direction it
/// runs: poem re-parses and still decides last, so a pair this wrongly *accepts*
/// is refused there, while a pair it wrongly *rejects* is a loud boot failure or
/// a `warn` that keeps the working certificate serving. Neither outcome is
/// silence, which is what the alternative — trusting the bytes — was.
///
/// A parse failure is described and never quoted: the parser's error carries
/// the line it choked on, byte for byte, and a PEM whose line breaks were
/// collapsed is one line holding all of it — the private key included — on its
/// way to the boot error or the renewal watcher's `warn`.
#[expect(
    clippy::map_err_ignore,
    reason = "the PEM parser's error quotes the line it choked on, which may hold the private key"
)]
fn validate_pair(cert: &[u8], key: &[u8]) -> Result<()> {
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
    // poem loads the listener's key with aws-lc-rs whatever provider the app
    // installed — that one reaches only its `ServerConfig` — so the key is loaded
    // with the same loader and the pair judged strictly: an aws-lc-rs key always
    // exposes its public half, so a mismatch can never pass as unknown.
    let signing = any_supported_type(&key)
        .context("the private key is not a type this build of rustls can sign with")?;
    CertifiedKey::new(chain, signing)
        .keys_match()
        .map_err(|error| match error {
            rustls::Error::InconsistentKeys(_) => anyhow::anyhow!(
                "the certificate and the private key do not correspond — the chain has to start \
                 with this key's own certificate, and a renewal that writes the two as separate \
                 operations is observable half-done; installing that pair fails every handshake"
            ),
            other => anyhow::anyhow!("the certificate cannot be read as X.509: {other}"),
        })?;
    Ok(())
}

/// What a PEM value has to look like, said instead of quoting one that does not.
const PEM_SHAPE: &str = "each -----BEGIN and -----END line stands on a line of its own, with \
     intact base64 between them, and a value whose line breaks were collapsed or escaped does \
     not parse";

/// Polls the PEM pair and reports a renewal. Polling rather than an OS watch
/// deliberately: a renewal is a minute-scale event, two `read`s a minute cost
/// nothing measurable, and it needs no new dependency and no per-platform
/// behaviour to reason about — including the case a file watcher gets wrong
/// most often, an atomic replace that swaps the inode out from under the watch.
struct Watcher {
    cert_path: PathBuf,
    key_path: PathBuf,
    cert: Vec<u8>,
    key: Vec<u8>,
    interval: Duration,
}

impl Watcher {
    /// Wait until the pair on disk differs from the pair in hand **and has
    /// stopped changing**, then hand back whatever it holds.
    ///
    /// A renewal tool that writes the certificate and its key as two operations
    /// is observable mid-write, and installing that pair serves a certificate
    /// its key cannot sign for — every handshake fails until the next tick.
    /// Requiring the pair to read back identical twice costs one extra interval
    /// on a renewal and closes that window.
    ///
    /// What settled is not yet known to be *usable*; that is
    /// [`next_renewal`](Self::next_renewal)'s question. Splitting the two is
    /// what lets each be tested on its own — the debounce against bytes that
    /// need not be certificates at all.
    async fn next_settled(&mut self) -> (Vec<u8>, Vec<u8>) {
        let mut pending: Option<(Vec<u8>, Vec<u8>)> = None;
        loop {
            tokio::time::sleep(self.interval).await;
            let (Some(cert), Some(key)) = (self.read(&self.cert_path), self.read(&self.key_path))
            else {
                // Half the pair is unreadable — a write in progress, most
                // likely. Whatever was pending is stale.
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
            // Read identical twice: the writer is done. Held as the pair in
            // hand whatever it turns out to be, so a pair that cannot serve is
            // reported once rather than every interval — any further write
            // differs from it and is read, settled and checked again.
            let (cert, key) = current;
            self.cert = cert.clone();
            self.key = key.clone();
            return (cert, key);
        }
    }

    /// The next settled pair that can actually serve.
    ///
    /// A settled read closes the window a mid-write leaves open. It does not
    /// close the one where both halves are written atomically and still do not
    /// correspond, nor the one where a file settles at zero bytes — so a settled
    /// pair is **checked** ([`validate_pair`]) before it is published.
    ///
    /// Publishing first and reporting success, which is what a renewal did
    /// before this check existed, turns either mistake into a total outage
    /// announced as an `info`. The refusal is a `warn` on `nest_rs::http`
    /// carrying both paths and the reason, and the working certificate keeps
    /// serving.
    async fn next_renewal(&mut self) -> RustlsConfig {
        loop {
            let (cert, key) = self.next_settled().await;
            if let Err(error) = validate_pair(&cert, &key) {
                tracing::warn!(
                    target: crate::target::HTTP,
                    cert = %self.cert_path.display(),
                    key = %self.key_path.display(),
                    error = format!("{error:#}"),
                    "renewed tls material was refused; keeping the certificate in use",
                );
                continue;
            }
            tracing::info!(
                target: crate::target::HTTP,
                cert = %self.cert_path.display(),
                key = %self.key_path.display(),
                "tls certificate renewed on disk",
            );
            return rustls_config(cert, key);
        }
    }

    /// A read failure is reported and skipped, never fatal: the process is
    /// already serving a valid certificate, and a renewal tool that unlinks
    /// before it writes would otherwise take the listener down with it.
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        // The loader's reader: a regular file of bounded size, opened so a FIFO
        // swapped in by a renewal tool cannot stall the watcher.
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

/// The refusal for half a pair: names the half as the deployment spelled it —
/// the [`Setting`] says whether that was the `_FILE` form — and both spellings
/// of the half it lacks.
/// Over a pair pinned in code the missing half is not missing, only not
/// replaced, and the sentence says that rather than claim there is none.
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
    use super::*;

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
        let (dir, fifo) = fifo("watcher");
        let (tx, rx) = std::sync::mpsc::channel();
        let watch = watcher(&dir);
        std::thread::spawn(move || {
            let _ = tx.send(watch.read(&fifo).is_none());
        });
        let skipped = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("returns instead of blocking on the pipe");
        assert!(skipped, "a FIFO is not re-read as material");
    }

    /// A watcher holding `cert`/`key` in hand and pointed at two paths, ticking
    /// fast enough for a test to outlast several intervals.
    fn watcher(dir: &std::path::Path) -> Watcher {
        Watcher {
            cert_path: dir.join("tls.pem"),
            key_path: dir.join("tls.key.pem"),
            cert: b"--IN-USE-CERT--".to_vec(),
            key: b"--IN-USE-KEY--".to_vec(),
            interval: Duration::from_millis(5),
        }
    }

    /// A directory the test owns, removed when the returned guard drops.
    ///
    /// The `pid` already prevents collisions; what this adds is not leaving one
    /// behind per run. Cleanup on `Drop` rather than at the end of the test, so
    /// a failing assertion does not leak either.
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
        // A renewal tool that unlinks before it writes leaves this window open
        // every single renewal. Treating it as fatal would take a listener down
        // that is serving a perfectly good certificate — so it is skipped, and
        // this line is the only thing that distinguishes "briefly mid-write"
        // from "the operator moved the file a week ago and TLS has been frozen
        // at the old certificate since".
        let logs = nest_rs_testing::LogCapture::install();
        let dir = scratch_dir("unreadable");
        let watcher = watcher(&dir);

        assert!(
            watcher.read(&watcher.cert_path).is_none(),
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
        // The failure this exists for is a **total outage announced as an
        // `info`**: publish first, report success, and every handshake from
        // then on fails against material that cannot present a chain. So the
        // check runs before the swap, the working certificate keeps serving,
        // and the renewal is reported as refused.
        let logs = nest_rs_testing::LogCapture::install();
        let dir = scratch_dir("refused");
        let mut watcher = watcher(&dir);
        std::fs::write(&watcher.cert_path, b"not a certificate").expect("write the cert half");
        std::fs::write(&watcher.key_path, b"not a key").expect("write the key half");

        // `next_renewal` loops until something serves, so nothing valid ever
        // arriving is the point: it must keep waiting rather than publish.
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

    // The derived `Debug` on `HttpConfig` would leak the key into logs — this
    // test pins the manual impl that redacts both the cert and key bytes.
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

    // `HttpTls::from_env` resolves through a `ConfigService`, so a map-backed
    // source feeds it hermetically — no process-env mutation, no `unsafe`, safe
    // under parallel test execution. Only the `_FILE` variant still needs a real
    // working directory, and `figment::Jail` supplies one.

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
        // The pinned-config path: an `HttpConfig` carrying TLS in code must not
        // lose it just because the environment says nothing about TLS.
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
        let cert = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/integration/fixtures/tls_a.pem"
        );
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

    // A half-configured TLS is refused even when the base already carries a
    // complete pair: the deployment clearly meant to swap the material, and
    // silently serving the pinned cert instead would hide the mistake.
    #[test]
    fn from_env_fails_on_a_half_config_even_over_a_complete_base() {
        let base = HttpTls::new(b"--PINNED-CERT--".to_vec(), b"--PINNED-KEY--".to_vec());
        assert!(HttpTls::from_env(&tls_env([("TLS_CERT", "--ENV-CERT--")]), Some(base),).is_err());
    }

    #[test]
    fn inline_material_is_not_watchable() {
        // There is no source to re-read, so `reload_secs` has nothing to mean.
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
        // The dual path: a base pinned in code still takes the deployment's
        // watch interval, per field, like every other `HttpConfig` key.
        let base = HttpTls::new(b"--CERT--".to_vec(), b"--KEY--".to_vec());
        let cfg = HttpTls::from_env(&tls_env([("TLS_RELOAD_SECS", "5")]), Some(base))
            .expect("no error")
            .expect("Some");
        assert_eq!(cfg.reload_secs, 5);
    }

    /// The watch interval has a ceiling like every duration a deployment sets:
    /// past a day it is refused naming the variable, from the environment and
    /// from a base pinned in code; `0` stays the off switch.
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

            // A pair with one half inline is not a watchable source: re-reading
            // the file half alone would pair a certificate with a stale key.
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
        // The debounce, stated as what it can actually guarantee: whatever the
        // watcher installs was read identical twice, so a file caught
        // mid-flush is never the thing that gets served.
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

        // Whichever step it settled on, it installed *that* step's pair — never
        // one step's certificate beside another's key.
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
        // The certificate is deliberately absent to start with: a renewal tool
        // that unlinks before it writes must not take the watcher down.
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

    /// A key whose line breaks were collapsed does not parse, and the parser's
    /// error carries that one line — the whole key — byte for byte. The refusal
    /// reaches the boot error and the watcher's `warn`, so it describes the
    /// problem and quotes nothing, not even as a list of bytes.
    #[test]
    fn a_pair_that_does_not_parse_is_described_and_never_quoted() {
        let cert_a = include_bytes!("../tests/integration/fixtures/tls_a.pem");
        let key_a = std::str::from_utf8(include_bytes!(
            "../tests/integration/fixtures/tls_a.key.pem"
        ))
        .expect("PEM is text");
        let secret = key_a.lines().nth(1).expect("a base64 line of the key");
        let as_bytes = secret.as_bytes()[..4]
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for collapsed in [key_a.replace('\n', " "), key_a.replace('\n', "\\n")] {
            let refused = validate_pair(cert_a, collapsed.as_bytes())
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

    /// A provider the app installs for its own reasons never loads the
    /// listener's key — poem loads that with aws-lc-rs — so one whose keys hide
    /// their public half cannot let a mismatched pair through the check, which
    /// would install cleanly and fail every handshake.
    #[test]
    fn a_mismatched_pair_is_refused_whatever_provider_the_app_installed() {
        use std::sync::Arc;

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

        let cert_a = include_bytes!("../tests/integration/fixtures/tls_a.pem");
        let key_b = include_bytes!("../tests/integration/fixtures/tls_b.key.pem");
        let refused = validate_pair(cert_a, key_b).expect_err("a mismatched pair is refused");
        assert!(
            format!("{refused:#}").contains("do not correspond"),
            "{refused:#}"
        );
    }

    /// And a key the installed provider cannot load at all is still one poem
    /// serves, so the check does not refuse it either.
    #[test]
    fn a_pair_poem_serves_is_accepted_whatever_provider_the_app_installed() {
        use std::sync::Arc;

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

        let cert_a = include_bytes!("../tests/integration/fixtures/tls_a.pem");
        let key_a = include_bytes!("../tests/integration/fixtures/tls_a.key.pem");
        validate_pair(cert_a, key_a).expect("a pair poem serves is not refused");
    }

    /// A key file holding no private key — empty, or a certificate in its
    /// place — says so, rather than blaming line breaks it does not have.
    #[test]
    fn a_key_file_holding_no_private_key_says_so() {
        let cert_a = include_bytes!("../tests/integration/fixtures/tls_a.pem");
        for (label, key) in [("empty", &b""[..]), ("a certificate", &cert_a[..])] {
            let refused =
                validate_pair(cert_a, key).expect_err("a file with no key in it is refused");
            assert!(
                format!("{refused:#}").contains("no PRIVATE KEY block"),
                "{label}: {refused:#}"
            );
        }
    }

    #[test]
    fn a_pair_that_cannot_serve_is_refused_before_it_is_published() {
        // Each of these installed cleanly and took TLS down for every name:
        // poem builds with `with_cert_resolver`, which — unlike
        // `with_single_cert` — never checks that the two halves correspond, and
        // an empty file parses as a chain with nothing in it.
        let cert_a = include_bytes!("../tests/integration/fixtures/tls_a.pem");
        let key_a = include_bytes!("../tests/integration/fixtures/tls_a.key.pem");
        let key_b = include_bytes!("../tests/integration/fixtures/tls_b.key.pem");

        validate_pair(cert_a, key_a).expect("the fixture pair corresponds");

        let mismatched = validate_pair(cert_a, key_b).expect_err("a mismatched pair is refused");
        assert!(
            format!("{mismatched:#}").contains("do not correspond"),
            "the refusal names what is wrong: {mismatched:#}",
        );

        let empty = validate_pair(b"", key_a).expect_err("an empty certificate is refused");
        assert!(
            format!("{empty:#}").contains("no CERTIFICATE block"),
            "the refusal names what is wrong: {empty:#}",
        );

        assert!(
            validate_pair(b"-----BEGIN CERTIFICATE-----\nnot base64\n", key_a).is_err(),
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
            // `Jail` runs in a fresh temp CWD; write the PEM files there and point
            // the `_FILE` vars at them by relative path.
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
