//! [`Store`] — the object store a [`StorageConfig`] names, opened through the
//! object_store backend its URL's scheme picks, and proved at boot: an `s3://`
//! store lists one page under its prefix, so a bucket that does not exist
//! refuses the boot instead of the first call. The same listing, one attempt
//! at a time, is what a health probe reads.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use http::StatusCode;
use nest_rs_config::{ClientTls, ConfigService, Namespaced, endpoint_shown};
use nest_rs_core::{Attempt, Backoff, Proving, Unproved, process_seed, prove};
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::client::{
    HttpClient, HttpConnector, HttpError, HttpRequest, HttpResponse, HttpService, ReqwestConnector,
};
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::prefix::PrefixStore;
use object_store::{BackoffConfig, ClientOptions, ObjectStore, RetryConfig};

use crate::backend::Backend;
use crate::config::{StorageConfig, is_plaintext};
use crate::error::{Result, StorageError};

/// The longest a dial to S3 waits — DNS, TCP and the TLS handshake; never
/// scaled down with the budget, since a dial cut shorter never connects.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Retries stop at half of `budget` and back off at most an eighth, so a call
/// whose attempts keep failing ends on the store's own error before the budget
/// cuts it.
pub(crate) fn retries_within(budget: Duration) -> RetryConfig {
    let max_backoff = budget / 8;
    RetryConfig {
        backoff: BackoffConfig {
            init_backoff: BackoffConfig::default().init_backoff.min(max_backoff),
            max_backoff,
            ..BackoffConfig::default()
        },
        retry_timeout: budget / 2,
        ..RetryConfig::default()
    }
}

/// The opened store: what every call goes through, what signs, and how a
/// sentence names it.
#[derive(Debug)]
pub(crate) struct Store {
    /// Every key's store, confined under the URL's prefix.
    pub(crate) objects: Arc<dyn ObjectStore>,
    /// What presigns, on a backend that can; it addresses the bucket's root,
    /// so a key is joined to [`prefix`](Self::prefix) first.
    signer: Option<AmazonS3>,
    /// What proves the store answers, on a backend with a server to reach:
    /// the boot's proof, then each health probe.
    probe: Option<Probe>,
    /// The prefix every key is addressed under.
    prefix: Path,
    /// The URL's scheme, naming the backend.
    pub(crate) backend: &'static str,
    /// The URL, as [`endpoint_shown`] shows it.
    pub(crate) url: String,
    /// The store as every sentence names it: the URL, and an S3 endpoint.
    pub(crate) shown: String,
}

impl Store {
    /// Open the store `config` names, refusing what the boot refuses, and prove
    /// an `s3://` one answers within the operation budget.
    pub(crate) async fn open(config: &StorageConfig) -> Result<Self> {
        let store = Self::built(config)?;
        tracing::info!(
            target: crate::TARGET,
            store = %store.shown,
            backend = store.backend,
            "opening the object store",
        );
        if let Some(probe) = &store.probe {
            proved(probe, config.operation_timeout, &store.shown).await?;
        }
        Ok(store)
    }

    /// The store `config` names, built and not yet proved.
    pub(crate) fn built(config: &StorageConfig) -> Result<Self> {
        let backend = config
            .backend()
            .map_err(|refused| StorageError::Init(Box::new(refused)))?;
        let url = endpoint_shown(&config.url);
        let (objects, signer, probe, prefix, shown) = match &backend {
            Backend::Memory { prefix } => (
                confined(InMemory::new(), prefix),
                None,
                None,
                prefix,
                url.clone(),
            ),
            Backend::S3 { bucket, prefix } => {
                let retries = retries_within(config.operation_timeout);
                let s3 = s3(config, bucket, ReqwestConnector::default(), retries)?;
                let probe = Probe::new(config, bucket, prefix)?;
                let shown = match &config.endpoint {
                    Some(endpoint) => format!("{url} at {}", endpoint_shown(endpoint)),
                    None => url.clone(),
                };
                (
                    confined(s3.clone(), prefix),
                    Some(s3),
                    Some(probe),
                    prefix,
                    shown,
                )
            }
        };
        Ok(Self {
            objects,
            signer,
            probe,
            prefix: prefix.clone(),
            backend: backend.scheme(),
            url,
            shown,
        })
    }

    /// Whether the store answers its listing now: one attempt of the boot's
    /// proof, read as the proof reads it — a store reached over verified TLS
    /// answers, even one refusing the listing itself (its credentials, or a
    /// policy granting no `s3:ListBucket`), as it boots, with the boot's
    /// `warn` filed again whenever the store starts refusing; an in-process
    /// store has nothing to reach.
    #[cfg(feature = "health")]
    pub(crate) async fn answers(&self) -> Result<()> {
        let Some(probe) = &self.probe else {
            return Ok(());
        };
        let env = ConfigService::for_namespace(StorageConfig::NAMESPACE);
        match probe.listed(&env, &self.shown).await {
            Ok(()) => Ok(()),
            Err(Attempt::Refused(refused)) => Err(refused),
            Err(Attempt::Failed(error)) => Err(StorageError::List {
                store: self.shown.clone(),
                source: Box::new(error),
            }),
        }
    }

    /// What presigns, or the refusal of a backend that cannot.
    pub(crate) fn signer(&self) -> Result<&AmazonS3> {
        self.signer.as_ref().ok_or(StorageError::Unsupported {
            operation: "presign",
            backend: self.backend,
        })
    }

    /// The in-process store `objects`, for a test of what reads through it.
    #[cfg(test)]
    pub(crate) fn over(objects: Arc<dyn ObjectStore>) -> Self {
        Self {
            objects,
            signer: None,
            probe: None,
            prefix: Path::default(),
            backend: "memory",
            url: "memory:///".to_owned(),
            shown: "memory:///".to_owned(),
        }
    }

    /// `key` as the bucket's root addresses it: under the URL's prefix.
    pub(crate) fn rooted(&self, key: &str) -> Path {
        self.prefix.parts().chain(Path::from(key).parts()).collect()
    }
}

/// `store`, confined under `prefix` when the URL names one.
fn confined<S: ObjectStore>(store: S, prefix: &Path) -> Arc<dyn ObjectStore> {
    if prefix.as_ref().is_empty() {
        Arc::new(store)
    } else {
        Arc::new(PrefixStore::new(store, prefix.clone()))
    }
}

/// The S3 client `config` describes for `bucket`, dialing through `connector`
/// and retrying a failed request as `retries` says.
fn s3<C: HttpConnector>(
    config: &StorageConfig,
    bucket: &str,
    connector: C,
    retries: RetryConfig,
) -> Result<AmazonS3> {
    let endpoint = config.endpoint.as_deref().unwrap_or_default();
    // `object_store`'s `with_allow_http` gates transfers only, so presigning
    // would mint a plaintext signed URL.
    if is_plaintext(endpoint) && !config.allow_http {
        return Err(StorageError::PlaintextEndpoint {
            endpoint: endpoint_shown(endpoint),
        });
    }
    // One `ClientOptions` value: `with_client_options` replaces the whole set,
    // so a builder call made before it would be undone.
    let options = ClientOptions::new()
        .with_allow_http(config.allow_http)
        // `object_store`'s default timeout bounds an attempt's body too,
        // cutting a download for its size; the budget and `download` do.
        .with_timeout_disabled()
        .with_connect_timeout(CONNECT_TIMEOUT);
    // Only the authorities `authorities_pem` hands over — the system's unless
    // the deployment named others — so every client the framework opens
    // trusts through one read of the store.
    let authorities = object_store::Certificate::from_pem_bundle(config.tls.authorities_pem())
        .map_err(|refused| StorageError::Init(Box::new(refused)))?;
    let options = authorities.into_iter().fold(
        options.with_no_system_certificates(true),
        ClientOptions::with_root_certificate,
    );
    let mut builder = AmazonS3Builder::new()
        .with_http_connector(connector)
        .with_client_options(options)
        .with_retry(retries)
        .with_region(&config.region)
        .with_access_key_id(&config.access_key)
        .with_secret_access_key(&config.secret_key)
        .with_bucket_name(bucket)
        .with_virtual_hosted_style_request(!config.force_path_style);
    if let Some(endpoint) = &config.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    builder
        .build()
        .map_err(|refused| StorageError::Init(Box::new(refused)))
}

/// How one listing of a bucket ended, short of a refusal.
enum Listed {
    /// The bucket answered its first page.
    Answered,
    /// The store was reached over verified TLS and refused to list it: the
    /// credentials, or a policy granting no `s3:ListBucket`.
    Forbidden(object_store::Error),
}

/// What lists a bucket to prove it answers, through a client of its own that
/// reads each answer's status: object_store does not say a listing's. That
/// client never retries, so each attempt's failure is classified the moment it
/// is met.
#[derive(Debug)]
struct Probe {
    client: AmazonS3,
    /// The status of the last answer the store gave the client.
    status: Arc<Mutex<Option<StatusCode>>>,
    /// Whether the store refused the last listing. Held across one listing,
    /// so the status read is that listing's (a readiness and a startup probe
    /// may ask at once) and a store refusing on every probe is said once.
    refusing: tokio::sync::Mutex<bool>,
    /// The prefix the store's keys live under, listed alone.
    prefix: Path,
}

impl Probe {
    /// The probe of `bucket` under `prefix`, dialing as `config` says.
    fn new(config: &StorageConfig, bucket: &str, prefix: &Path) -> Result<Self> {
        let status = Arc::new(Mutex::new(None));
        let client = s3(
            config,
            bucket,
            Witnessed {
                status: Arc::clone(&status),
            },
            RetryConfig {
                max_retries: 0,
                ..RetryConfig::default()
            },
        )?;
        Ok(Self {
            client,
            status,
            refusing: tokio::sync::Mutex::new(false),
            prefix: prefix.clone(),
        })
    }

    /// List one page under the prefix, once: `Ok` when the store answered it
    /// or refused it, and a refusal is said at the listing that first meets
    /// it — the boot's, or the probe's after the store stopped listing.
    async fn listed(
        &self,
        env: &ConfigService,
        shown: &str,
    ) -> std::result::Result<(), Attempt<object_store::Error, StorageError>> {
        let mut refusing = self.refusing.lock().await;
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let prefix = (!self.prefix.as_ref().is_empty()).then_some(&self.prefix);
        let listed = match self.client.list(prefix).next().await {
            Some(Err(error)) => {
                let answered = *self.status.lock().unwrap_or_else(PoisonError::into_inner);
                classify(error, answered, env, shown)
            }
            _ => Ok(Listed::Answered),
        };
        let was_refusing =
            std::mem::replace(&mut *refusing, matches!(listed, Ok(Listed::Forbidden(_))));
        match listed? {
            Listed::Forbidden(error) if !was_refusing => tracing::warn!(
                target: crate::TARGET,
                store = %shown,
                credentials = %format!(
                    "{} and {}",
                    env.var_name("ACCESS_KEY"),
                    env.var_name("SECRET_KEY")
                ),
                error = %nest_rs_core::error_message(&error),
                "the object store refused to list its bucket — its credentials are refused, or \
                 its policy grants no s3:ListBucket on the prefix; read as answering, and the \
                 next call will say which",
            ),
            Listed::Answered | Listed::Forbidden(_) => {}
        }
        Ok(())
    }
}

/// Prove the bucket answers one page of its listing under its prefix within
/// `budget`, through `probe`.
async fn proved(probe: &Probe, budget: Duration, shown: &str) -> Result<()> {
    let env = ConfigService::for_namespace(StorageConfig::NAMESPACE);
    let listed = prove(
        budget,
        Backoff::RECONNECT,
        process_seed(),
        said(shown, budget),
        |_left| probe.listed(&env, shown),
    )
    .await;
    match listed {
        Ok(()) => Ok(()),
        Err(Unproved::Refused(refused)) => Err(refused),
        Err(Unproved::Spent { last, .. }) => Err(StorageError::Unreachable {
            store: shown.to_owned(),
            budget,
            source: last.map(|error| Box::new(error) as _),
        }),
    }
}

/// What one failed listing met: a refusal every attempt would repeat, a
/// store that refused the credentials, or a failure that may clear — no
/// answer, or a status the store answers while it recovers.
fn classify(
    error: object_store::Error,
    answered: Option<StatusCode>,
    env: &ConfigService,
    shown: &str,
) -> std::result::Result<Listed, Attempt<object_store::Error, StorageError>> {
    let reason = match answered {
        Some(StatusCode::NOT_FOUND) => format!(
            "the bucket {} names does not exist — create it, or name one that does",
            env.var_name("URL")
        ),
        Some(StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED) => {
            return Ok(Listed::Forbidden(error));
        }
        Some(status) if status.is_redirection() => format!(
            "the store answered {status}, sending the bucket elsewhere — set {} to the bucket's \
             region, or {} and {} to the store that serves it",
            env.var_name("REGION"),
            env.var_name("ENDPOINT"),
            env.var_name("FORCE_PATH_STYLE"),
        ),
        Some(status) if !(status.is_success() || may_clear(status)) => format!(
            "the store answered {status}, as it would every attempt — check that {}, {} and {} \
             are the store's",
            env.var_name("REGION"),
            env.var_name("ENDPOINT"),
            env.var_name("FORCE_PATH_STYLE"),
        ),
        None if ClientTls::negotiation_failed(&error) => {
            ClientTls::remedy(env, "the object store", "ENDPOINT", &error)
        }
        _ => return Err(Attempt::Failed(error)),
    };
    Err(Attempt::Refused(StorageError::Refused {
        store: shown.to_owned(),
        reason,
        source: Box::new(error),
    }))
}

/// Whether a store answering `status` may answer otherwise on a later attempt:
/// it is failing, timed out or throttling.
fn may_clear(status: StatusCode) -> bool {
    status.is_server_error()
        || matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
        )
}

/// What the proof of `store` reports, said on this crate's target.
fn said(store: &str, budget: Duration) -> impl FnMut(Proving<'_, object_store::Error>) + '_ {
    move |proving| match proving {
        Proving::Retrying { attempt, error } => tracing::warn!(
            target: crate::TARGET,
            store = %store,
            attempt,
            error = %nest_rs_core::error_message(error),
            "object store unreachable — retrying within the operation budget",
        ),
        Proving::TimedOut { attempt } => tracing::warn!(
            target: crate::TARGET,
            store = %store,
            attempt,
            timeout_secs = budget.as_secs_f64(),
            "object store proof timed out",
        ),
        Proving::Proved { attempts } if attempts > 1 => tracing::info!(
            target: crate::TARGET,
            store = %store,
            attempts,
            "object store reached after retrying",
        ),
        _ => {}
    }
}

/// The HTTP connector of the boot's proof: object_store's own, keeping the
/// status of the last answer the store gave.
#[derive(Debug)]
struct Witnessed {
    status: Arc<Mutex<Option<StatusCode>>>,
}

impl HttpConnector for Witnessed {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(Witness {
            client: ReqwestConnector::default().connect(options)?,
            status: Arc::clone(&self.status),
        }))
    }
}

/// One client of [`Witnessed`]'s.
#[derive(Debug)]
struct Witness {
    client: HttpClient,
    status: Arc<Mutex<Option<StatusCode>>>,
}

#[async_trait::async_trait]
impl HttpService for Witness {
    async fn call(&self, request: HttpRequest) -> std::result::Result<HttpResponse, HttpError> {
        let answer = self.client.execute(request).await?;
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer.status());
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU16, Ordering};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    /// What a store refusing its listing is said as, on this crate's target.
    const REFUSED_TO_LIST: &str = "the object store refused to list its bucket — its credentials \
         are refused, or its policy grants no s3:ListBucket on the prefix; read as answering, and \
         the next call will say which";

    #[test]
    fn the_last_backoff_ends_within_the_budget_whatever_the_budget() {
        for budget in [
            Duration::from_millis(1),
            Duration::from_secs(1),
            StorageConfig::default().operation_timeout,
            Duration::from_secs(60 * 60),
        ] {
            let retries = retries_within(budget);
            assert!(retries.backoff.init_backoff <= retries.backoff.max_backoff);
            assert!(
                retries.retry_timeout + retries.backoff.max_backoff < budget,
                "{budget:?}"
            );
        }
    }

    /// An empty page of a bucket's listing, as S3 answers it.
    const EMPTY_PAGE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
        <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
        <Name>bucket</Name><Prefix></Prefix><KeyCount>0</KeyCount><MaxKeys>1000</MaxKeys>\
        <IsTruncated>false</IsTruncated></ListBucketResult>";

    /// What an S3 double heard: the request line of each request it read.
    #[derive(Clone, Default)]
    struct Heard(Arc<Mutex<Vec<String>>>);

    impl Heard {
        fn lines(&self) -> Vec<String> {
            self.0.lock().expect("heard").clone()
        }
    }

    /// An S3 on loopback presenting a certificate `authority` issued, answering
    /// a bucket's listing with `listing`'s status — an empty page on `200` —
    /// and every other request `404`.
    async fn store_presenting(
        authority: &nest_rs_testing::TestAuthority,
        listing: u16,
    ) -> (String, Heard) {
        store_answering(authority, Arc::new(AtomicU16::new(listing))).await
    }

    /// [`store_presenting`], answering a listing with the status `listing`
    /// holds when its connection arrives — each answer closes its connection,
    /// so that is the listing's own.
    async fn store_answering(
        authority: &nest_rs_testing::TestAuthority,
        listing: Arc<AtomicU16>,
    ) -> (String, Heard) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let addr = listener.local_addr().expect("its address");
        let acceptor = authority.server(&["127.0.0.1"]).acceptor(None);
        let heard = Heard::default();
        let kept = heard.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, kept) = (acceptor.clone(), kept.clone());
                let listing = listing.load(Ordering::SeqCst);
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut head = Vec::new();
                    let mut buf = [0_u8; 4096];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&buf[..read]),
                        }
                    }
                    let line = String::from_utf8_lossy(&head)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    let (status, body) = if line.contains("list-type=2") {
                        (listing, if listing == 200 { EMPTY_PAGE } else { "" })
                    } else {
                        (404, "")
                    };
                    kept.0.lock().expect("heard").push(line);
                    let answer = format!(
                        "HTTP/1.1 {status} Answered\r\ncontent-length: {}\r\nconnection: \
                         close\r\n\r\n{body}",
                        body.len()
                    );
                    if tls.write_all(answer.as_bytes()).await.is_ok() {
                        tls.shutdown().await.ok();
                    }
                });
            }
        });
        (format!("https://{addr}"), heard)
    }

    fn reaching(endpoint: &str, url: &str, tls: ClientTls) -> StorageConfig {
        StorageConfig {
            url: url.into(),
            endpoint: Some(endpoint.into()),
            access_key: "AKIATEST".into(),
            secret_key: "test-secret".into(),
            operation_timeout: Duration::from_secs(2),
            tls,
            ..StorageConfig::default()
        }
    }

    #[tokio::test]
    async fn a_bucket_answering_its_listing_is_proved_with_one_request() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 200).await;
        Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
            .await
            .expect("the bucket answers");
        let lines = heard.lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("GET /bucket?list-type=2"), "{lines:?}");
    }

    #[tokio::test]
    async fn a_prefixed_url_proves_the_listing_of_its_prefix_alone() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 200).await;
        Store::open(&reaching(
            &endpoint,
            "s3://bucket/tenants/acme",
            authority.client_tls(),
        ))
        .await
        .expect("the prefix answers");
        let lines = heard.lines();
        assert!(
            lines.len() == 1 && lines[0].contains("prefix=tenants%2Facme%2F"),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn a_bucket_that_does_not_exist_refuses_at_once_naming_the_url_variable() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 404).await;
        let refused = Store::open(&reaching(&endpoint, "s3://absent", authority.client_tls()))
            .await
            .expect_err("no bucket to prove");
        assert!(
            matches!(refused, StorageError::Refused { .. }),
            "{refused:?}"
        );
        let chain = nest_rs_core::error_message(&refused);
        assert!(
            chain.contains(&format!(
                "the bucket {} names does not exist",
                nest_rs_config::var_name("storage", "URL")
            )) && chain.contains("s3://absent at"),
            "{chain}"
        );
        assert_eq!(heard.lines().len(), 1, "never retried");
    }

    #[tokio::test]
    async fn a_store_refusing_the_credentials_boots_with_one_warn_naming_them() {
        let authority = nest_rs_testing::TestAuthority::new();
        let logs = nest_rs_testing::LogCapture::install();
        for status in [403, 401] {
            let (endpoint, _) = store_presenting(&authority, status).await;
            Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
                .await
                .expect("a least-privilege policy is no reason to refuse the boot");
        }
        let warned = logs.find(crate::TARGET, REFUSED_TO_LIST);
        assert_eq!(warned.len(), 2, "one per boot: {:#?}", logs.events());
        for warn in warned {
            assert_eq!(warn.level, "warn");
            let credentials = warn.field("credentials").unwrap_or_default();
            assert!(
                credentials.contains(&nest_rs_config::var_name("storage", "ACCESS_KEY")),
                "{credentials}"
            );
        }
    }

    #[tokio::test]
    async fn a_certificate_no_trusted_authority_signed_refuses_at_once_naming_the_remedy() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 200).await;
        let started = tokio::time::Instant::now();
        let refused = Store::open(&reaching(&endpoint, "s3://bucket", ClientTls::default()))
            .await
            .expect_err("a certificate no trusted authority signed is not talked to");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        assert!(
            heard.lines().is_empty(),
            "no request crossed the failed handshake"
        );
        let chain = nest_rs_core::error_message(&refused);
        assert!(
            matches!(refused, StorageError::Refused { .. })
                && chain.contains(&nest_rs_config::var_name("storage", "TLS_CA_CERT"))
                && chain.contains("UnknownIssuer"),
            "{chain}"
        );
    }

    #[tokio::test]
    async fn a_redirect_without_a_location_refuses_at_once_naming_the_region_and_the_endpoint() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 301).await;
        let started = tokio::time::Instant::now();
        let refused = Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
            .await
            .expect_err("a store sending the bucket elsewhere is not proved");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(heard.lines().len(), 1, "never retried");
        let chain = nest_rs_core::error_message(&refused);
        assert!(
            matches!(refused, StorageError::Refused { .. })
                && chain.contains("the store answered 301 Moved Permanently")
                && chain.contains(&nest_rs_config::var_name("storage", "REGION"))
                && chain.contains(&nest_rs_config::var_name("storage", "ENDPOINT"))
                && chain.contains(&nest_rs_config::var_name("storage", "FORCE_PATH_STYLE"))
                && chain.contains("redirected without a location"),
            "{chain}"
        );
    }

    #[tokio::test]
    async fn a_status_every_attempt_would_meet_refuses_at_once_saying_it() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 400).await;
        let started = tokio::time::Instant::now();
        let refused = Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
            .await
            .expect_err("a store answering 400 is not proved");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(heard.lines().len(), 1, "never retried");
        let chain = nest_rs_core::error_message(&refused);
        assert!(
            matches!(refused, StorageError::Refused { .. })
                && chain.contains("the store answered 400 Bad Request, as it would every attempt")
                && chain.contains(&nest_rs_config::var_name("storage", "REGION")),
            "{chain}"
        );
    }

    #[tokio::test]
    async fn a_store_failing_in_ways_that_may_clear_is_retried_then_said_unreachable() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 500).await;
        let logs = nest_rs_testing::LogCapture::install();
        let budget = Duration::from_millis(400);
        let refused = Store::open(&StorageConfig {
            operation_timeout: budget,
            ..reaching(&endpoint, "s3://bucket", authority.client_tls())
        })
        .await
        .expect_err("a store answering 500 is never proved");
        assert!(
            matches!(refused, StorageError::Unreachable { budget: within, .. } if within == budget),
            "{refused:?}"
        );
        assert!(heard.lines().len() > 1, "retried");
        assert!(
            !logs
                .find(
                    crate::TARGET,
                    "object store unreachable — retrying within the operation budget",
                )
                .is_empty()
                || !logs
                    .find(crate::TARGET, "object store proof timed out")
                    .is_empty(),
            "{:#?}",
            logs.events()
        );
    }

    /// The store a probe reads, built without the boot's proof.
    fn unproved(endpoint: &str, authority: &nest_rs_testing::TestAuthority) -> Store {
        Store::built(&reaching(endpoint, "s3://bucket", authority.client_tls()))
            .expect("the store builds")
    }

    #[tokio::test]
    async fn a_probe_reads_a_bucket_answering_its_listing_with_one_request() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 200).await;
        unproved(&endpoint, &authority)
            .answers()
            .await
            .expect("the bucket answers");
        assert_eq!(heard.lines().len(), 1, "{:?}", heard.lines());
    }

    #[tokio::test]
    async fn a_probe_reads_a_store_refusing_the_listing_itself_as_answering_as_the_boot_does() {
        let authority = nest_rs_testing::TestAuthority::new();
        for status in [403, 401] {
            let (endpoint, _) = store_presenting(&authority, status).await;
            unproved(&endpoint, &authority)
                .answers()
                .await
                .expect("a least-privilege policy is no reason to turn traffic away");
        }
    }

    #[tokio::test]
    async fn a_store_starting_to_refuse_the_listing_is_said_once_by_the_probe_that_meets_it() {
        let authority = nest_rs_testing::TestAuthority::new();
        let listing = Arc::new(AtomicU16::new(403));
        let (endpoint, _) = store_answering(&authority, Arc::clone(&listing)).await;
        let logs = nest_rs_testing::LogCapture::install();
        let store = Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
            .await
            .expect("a least-privilege policy is no reason to refuse the boot");
        assert_eq!(
            logs.find(crate::TARGET, REFUSED_TO_LIST).len(),
            1,
            "the boot's"
        );
        for (status, said) in [(403, 1), (200, 1), (401, 2), (401, 2), (500, 2), (403, 3)] {
            listing.store(status, Ordering::SeqCst);
            let answered = store.answers().await;
            assert_eq!(answered.is_ok(), status != 500, "{status}: {answered:?}");
            let warned = logs.find(crate::TARGET, REFUSED_TO_LIST);
            assert_eq!(warned.len(), said, "after {status}: {:#?}", logs.events());
        }
        for warn in logs.find(crate::TARGET, REFUSED_TO_LIST) {
            assert_eq!(warn.level, "warn");
            let credentials = warn.field("credentials").unwrap_or_default();
            assert!(
                credentials.contains(&nest_rs_config::var_name("storage", "ACCESS_KEY"))
                    && credentials.contains(&nest_rs_config::var_name("storage", "SECRET_KEY")),
                "{credentials}"
            );
            assert!(warn.field("error").is_some_and(|chain| !chain.is_empty()));
        }
    }

    #[tokio::test]
    async fn a_probe_reads_a_missing_bucket_as_the_boot_refuses_it() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 404).await;
        let refused = unproved(&endpoint, &authority)
            .answers()
            .await
            .expect_err("no bucket to list");
        assert!(
            matches!(refused, StorageError::Refused { .. }),
            "{refused:?}"
        );
        assert!(
            nest_rs_core::error_message(&refused).contains(&format!(
                "the bucket {} names does not exist",
                nest_rs_config::var_name("storage", "URL")
            )),
            "{refused}"
        );
        assert_eq!(heard.lines().len(), 1, "never retried");
    }

    #[tokio::test]
    async fn a_probe_reads_a_failing_store_once_naming_the_listing() {
        let authority = nest_rs_testing::TestAuthority::new();
        let (endpoint, heard) = store_presenting(&authority, 500).await;
        let failed = unproved(&endpoint, &authority)
            .answers()
            .await
            .expect_err("a store answering 500 does not answer");
        assert!(matches!(failed, StorageError::List { .. }), "{failed:?}");
        assert_eq!(
            heard.lines().len(),
            1,
            "a probe is one attempt: the orchestrator asks again"
        );
    }

    #[tokio::test]
    async fn memory_opens_without_a_request_and_confines_keys_under_its_prefix() {
        nest_rs_testing::load_project_env();
        let store = Store::open(&StorageConfig {
            url: "memory:///tenants/acme".into(),
            ..StorageConfig::default()
        })
        .await
        .expect("memory opens in a test profile");
        assert_eq!(store.rooted("a.png"), Path::from("tenants/acme/a.png"));
        assert!(store.signer().is_err());
    }
}
