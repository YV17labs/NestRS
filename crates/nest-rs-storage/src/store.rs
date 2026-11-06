//! [`Store`] — the object store a [`StorageConfig`] names, opened through the
//! object_store backend its URL's scheme picks, and proved at boot: an `s3://`
//! store lists one page under its prefix, so a bucket that does not exist
//! refuses the boot instead of the first call. The same listing, one attempt
//! at a time, is what a health probe reads.

use std::cell::Cell;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use http::StatusCode;
use nest_rs_config::{ClientTls, ConfigService, Namespaced, endpoint_shown};
use nest_rs_core::{Attempt, Backoff, Proving, Unproved, process_seed, prove};
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::client::{
    HttpClient, HttpConnector, HttpError, HttpRequest, HttpResponse, HttpResponseBody, HttpService,
    ReqwestConnector,
};
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::prefix::PrefixStore;
use object_store::{BackoffConfig, ClientOptions, ObjectStore, RetryConfig};

use crate::backend::Backend;
use crate::config::{StorageConfig, is_plaintext};
use crate::error::{Exchange, Result, StorageError};

/// The longest a dial to S3 waits — DNS, TCP and the TLS handshake; never
/// scaled down with the budget, since a dial cut shorter never connects.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How much of a refusal is read for what S3's error document says: far more
/// than any such document holds, and a store sending more says nothing by it.
const REFUSAL_DOCUMENT_BOUND: usize = 64 * 1024;

/// The codes S3 answers for a key it rejects before it reads any policy: one it
/// does not know, a signature its secret does not make, a security token it does
/// not accept. Every call would fail alike, so no policy explains them away.
const KEY_REJECTED: [&str; 3] = [
    "InvalidAccessKeyId",
    "SignatureDoesNotMatch",
    "InvalidSecurity",
];

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
            config.warn_when_open();
            proved(probe, config.operation_timeout, &store.shown).await?;
        }
        Ok(store)
    }

    /// The store `config` names, built and not yet proved.
    pub(crate) fn built(config: &StorageConfig) -> Result<Self> {
        Self::dialing(config, ReqwestConnector::default())
    }

    /// The store `config` names, built and not yet proved, its calls to an
    /// `s3://` store dialing through `connector`.
    pub(crate) fn dialing<C: HttpConnector>(config: &StorageConfig, connector: C) -> Result<Self> {
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
                let s3 = s3(config, bucket, connector, retries)?;
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
    /// answers, even one whose policy grants no `s3:ListBucket`, as it boots,
    /// with the boot's `warn` filed again whenever the store starts refusing;
    /// a store rejecting the key is refused, as the boot refuses it; an
    /// in-process store has nothing to reach.
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

/// The S3 client `config` describes for `bucket`, dialing through `connector`,
/// each exchange [`Witnessed`], and retrying a failed request as `retries`
/// says.
fn s3<C: HttpConnector>(
    config: &StorageConfig,
    bucket: &str,
    connector: C,
    retries: RetryConfig,
) -> Result<AmazonS3> {
    let endpoint = config.endpoint.as_deref().unwrap_or_default();
    let allow_http = config.allow_http.is_open();
    // `object_store`'s `with_allow_http` gates transfers only, so presigning
    // would mint a plaintext signed URL.
    if is_plaintext(endpoint) && !allow_http {
        return Err(StorageError::PlaintextEndpoint {
            endpoint: endpoint_shown(endpoint),
        });
    }
    // One `ClientOptions` value: `with_client_options` replaces the whole set,
    // so a builder call made before it would be undone.
    let options = ClientOptions::new()
        .with_allow_http(allow_http)
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
        .with_http_connector(Witnessed(connector))
        .with_client_options(options)
        .with_retry(retries)
        .with_region(&config.region)
        .with_access_key_id(config.access_key.expose_secret())
        .with_secret_access_key(config.secret_key.expose_secret())
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
    /// The store was reached over verified TLS and its policy refused to list
    /// the bucket: no `s3:ListBucket`, or no grant at all on a key it knows,
    /// which S3 answers alike.
    Forbidden(object_store::Error),
}

/// What lists a bucket to prove it answers, through a client of its own, and
/// reads the status of its answer off [`witnessed`]: object_store does not say
/// a listing's. That client never retries, so each attempt's failure is
/// classified the moment it is met.
#[derive(Debug)]
struct Probe {
    client: AmazonS3,
    /// Whether the store refused the last listing. Held across one listing,
    /// so a store refusing on every probe is said once, even when a readiness
    /// and a startup probe ask at once.
    refusing: tokio::sync::Mutex<bool>,
    /// The prefix the store's keys live under, listed alone.
    prefix: Path,
}

impl Probe {
    /// The probe of `bucket` under `prefix`, dialing as `config` says.
    fn new(config: &StorageConfig, bucket: &str, prefix: &Path) -> Result<Self> {
        let client = s3(
            config,
            bucket,
            ReqwestConnector::default(),
            RetryConfig {
                max_retries: 0,
                ..RetryConfig::default()
            },
        )?;
        Ok(Self {
            client,
            refusing: tokio::sync::Mutex::new(false),
            prefix: prefix.clone(),
        })
    }

    /// List one page under the prefix, once: `Ok` when the store answered it
    /// or its policy refused it, and that refusal is said at the listing that
    /// first meets it — the boot's, or the probe's after the store stopped
    /// listing.
    async fn listed(
        &self,
        env: &ConfigService,
        shown: &str,
    ) -> std::result::Result<(), Attempt<object_store::Error, StorageError>> {
        let mut refusing = self.refusing.lock().await;
        let prefix = (!self.prefix.as_ref().is_empty()).then_some(&self.prefix);
        let listed = match witnessed(self.client.list(prefix).next()).await {
            (Some(Err(error)), last) => classify(error, last.as_ref(), env, shown),
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
                    env.var_name(StorageConfig::ACCESS_KEY.key()),
                    env.var_name(StorageConfig::SECRET_KEY.key())
                ),
                error = %nest_rs_core::error_message(&error),
                "the object store refused to list its bucket — its policy grants these \
                 credentials no s3:ListBucket on the prefix; read as answering, and the next call \
                 will say whether it grants what the app does",
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
/// store whose policy refused the listing, or a failure that may clear — no
/// answer, or a status the store answers while it recovers.
fn classify(
    error: object_store::Error,
    last: Option<&Exchange>,
    env: &ConfigService,
    shown: &str,
) -> std::result::Result<Listed, Attempt<object_store::Error, StorageError>> {
    let reason = match last {
        Some(rejected @ Exchange::KeyRejected { .. }) => format!(
            "{rejected}, as it would on every call — set {} and {} to a pair it knows",
            env.var_name(StorageConfig::ACCESS_KEY.key()),
            env.var_name(StorageConfig::SECRET_KEY.key()),
        ),
        Some(Exchange::Answered(StatusCode::NOT_FOUND)) => format!(
            "the bucket {} names does not exist — create it, or name one that does",
            env.var_name(StorageConfig::URL.key())
        ),
        Some(Exchange::Answered(StatusCode::FORBIDDEN)) => return Ok(Listed::Forbidden(error)),
        Some(Exchange::Answered(status)) if status.is_redirection() => format!(
            "the store answered {status}, sending the bucket elsewhere — set {} to the bucket's \
             region, or {} and {} to the store that serves it",
            env.var_name(StorageConfig::REGION.key()),
            env.var_name(StorageConfig::ENDPOINT.key()),
            env.var_name(StorageConfig::FORCE_PATH_STYLE.key()),
        ),
        Some(Exchange::Answered(status)) if !(status.is_success() || may_clear(*status)) => {
            format!(
                "the store answered {status}, as it would every attempt — check that {}, {} and \
                 {} are the store's",
                env.var_name(StorageConfig::REGION.key()),
                env.var_name(StorageConfig::ENDPOINT.key()),
                env.var_name(StorageConfig::FORCE_PATH_STYLE.key()),
            )
        }
        Some(Exchange::Unanswered { .. }) | None if ClientTls::negotiation_failed(&error) => {
            ClientTls::remedy(
                env,
                "the object store",
                StorageConfig::ENDPOINT.key(),
                &error,
            )
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

tokio::task_local! {
    /// How the last exchange of the unit [`witnessed`] runs ended.
    static LAST_EXCHANGE: Cell<Option<Exchange>>;
}

/// `unit`'s output, and how its last exchange with the store ended, as every
/// S3 client of the store keeps it ([`Witnessed`]).
pub(crate) async fn witnessed<F: Future>(unit: F) -> (F::Output, Option<Exchange>) {
    LAST_EXCHANGE
        .scope(Cell::new(None), async {
            let output = unit.await;
            (output, LAST_EXCHANGE.with(Cell::take))
        })
        .await
}

/// The HTTP connector of every S3 client the store opens: `C`'s, keeping how
/// each exchange ended for the unit [`witnessed`] runs it in.
#[derive(Debug)]
struct Witnessed<C>(C);

impl<C: HttpConnector> HttpConnector for Witnessed<C> {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(Witness(self.0.connect(options)?)))
    }
}

/// One client of [`Witnessed`]'s.
#[derive(Debug)]
struct Witness(HttpClient);

#[async_trait::async_trait]
impl HttpService for Witness {
    async fn call(&self, request: HttpRequest) -> std::result::Result<HttpResponse, HttpError> {
        let (answer, exchange) = match self.0.execute(request).await {
            Ok(answered) => {
                let (answered, exchange) = read(answered).await;
                (Ok(answered), exchange)
            }
            Err(failed) => {
                let exchange = Exchange::unanswered(&failed);
                (Err(failed), exchange)
            }
        };
        // An exchange outside a witnessed unit has no reader: object_store's
        // own resume of a download's body, read after its call returned.
        LAST_EXCHANGE.try_with(|last| last.set(Some(exchange))).ok();
        answer
    }
}

/// `answered`, and the exchange it ended: a refusal's error document is read and
/// handed back, so object_store still meets the answer it was sent; every other
/// answer's body is left unread, a download's above all.
async fn read(answered: HttpResponse) -> (HttpResponse, Exchange) {
    let status = answered.status();
    if !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return (answered, Exchange::Answered(status));
    }
    let (parts, body) = answered.into_parts();
    let document = refusal_document(body).await;
    let code = key_rejected_by(&document);
    let exchange = if code.is_some() || status == StatusCode::UNAUTHORIZED {
        Exchange::KeyRejected { status, code }
    } else {
        Exchange::Answered(status)
    };
    (HttpResponse::from_parts(parts, document.into()), exchange)
}

/// What a refusal's body holds, up to [`REFUSAL_DOCUMENT_BOUND`]: nothing when
/// it holds more, and what arrived when it stops short.
async fn refusal_document(body: HttpResponseBody) -> bytes::Bytes {
    let mut document = Vec::new();
    let mut chunks = body.bytes_stream();
    while let Some(Ok(chunk)) = chunks.next().await {
        document.extend_from_slice(&chunk);
        if document.len() > REFUSAL_DOCUMENT_BOUND {
            return bytes::Bytes::new();
        }
    }
    document.into()
}

/// The [`KEY_REJECTED`] code `document`'s `<Code>` holds, S3's error document
/// naming its error first and in that one element.
fn key_rejected_by(document: &[u8]) -> Option<&'static str> {
    let (_, code) = std::str::from_utf8(document).ok()?.split_once("<Code>")?;
    let (code, _) = code.split_once("</Code>")?;
    KEY_REJECTED.into_iter().find(|rejected| *rejected == code)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU16, Ordering};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    /// What a store refusing its listing is said as, on this crate's target.
    const REFUSED_TO_LIST: &str = "the object store refused to list its bucket — its policy \
         grants these credentials no s3:ListBucket on the prefix; read as answering, and the next \
         call will say whether it grants what the app does";

    /// The access key a refusal's document echoes, which no sentence repeats.
    const ECHOED_KEY: &str = "AKIAECHOED";

    /// Every answer that says the store rejects the key itself.
    fn rejected_keys() -> [(u16, String); 4] {
        [
            (403, error_document("InvalidAccessKeyId")),
            (403, error_document("SignatureDoesNotMatch")),
            (403, error_document("InvalidSecurity")),
            (401, String::new()),
        ]
    }

    /// Every answer a least-privilege policy or a proxy gives, or that says
    /// nothing of the key: an unknown code never refuses a boot.
    fn policy_refusals() -> [(u16, String); 4] {
        [
            (403, error_document("AccessDenied")),
            (403, error_document("SomeVendorsOwnCode")),
            (403, "<html>forbidden by the gateway</html>".to_owned()),
            (403, String::new()),
        ]
    }

    #[test]
    fn a_refusal_names_a_key_rejected_only_by_s3s_own_code_in_its_own_element() {
        for (document, rejected) in [
            (
                error_document("InvalidAccessKeyId"),
                Some("InvalidAccessKeyId"),
            ),
            (
                error_document("SignatureDoesNotMatch"),
                Some("SignatureDoesNotMatch"),
            ),
            (error_document("InvalidSecurity"), Some("InvalidSecurity")),
            (error_document("AccessDenied"), None),
            (error_document("SomeVendorsOwnCode"), None),
            (error_document("signaturedoesnotmatch"), None),
            ("<Error><Code>SignatureDoesNotMatch".to_owned(), None),
            (
                "<Error><Message>SignatureDoesNotMatch</Message></Error>".to_owned(),
                None,
            ),
            (String::new(), None),
        ] {
            assert_eq!(key_rejected_by(document.as_bytes()), rejected, "{document}");
        }
        assert_eq!(key_rejected_by(&[0xff, 0xfe]), None, "not a document");
    }

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

    /// What an S3 double heard: the request line of each request it read, and
    /// how many connections it accepted — a refusal at once is one, whatever
    /// the handshake or the machine's load costs it.
    #[derive(Clone, Default)]
    struct Heard(Arc<Mutex<Vec<String>>>, Arc<std::sync::atomic::AtomicUsize>);

    impl Heard {
        fn lines(&self) -> Vec<String> {
            self.0.lock().expect("heard").clone()
        }

        fn connections(&self) -> usize {
            self.1.load(std::sync::atomic::Ordering::SeqCst)
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
        store_refusing(authority, listing, String::new()).await
    }

    /// The error document S3 answers a refusal with, echoing the key it was
    /// signed with, as `SignatureDoesNotMatch` does.
    fn error_document(code: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code>\
             <Message>refused</Message><AWSAccessKeyId>{ECHOED_KEY}</AWSAccessKeyId>\
             <RequestId>1</RequestId></Error>"
        )
    }

    /// [`store_answering`], whose answer to a listing it does not serve carries
    /// `document`.
    async fn store_refusing(
        authority: &nest_rs_testing::TestAuthority,
        listing: Arc<AtomicU16>,
        document: String,
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
                kept.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (acceptor, kept) = (acceptor.clone(), kept.clone());
                let listing = listing.load(Ordering::SeqCst);
                let document = document.clone();
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
                        (
                            listing,
                            if listing == 200 {
                                EMPTY_PAGE
                            } else {
                                document.as_str()
                            },
                        )
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

    /// Opening the store is where a deployment's plain HTTP takes effect, so the
    /// line is filed there, whether or not the store then answers.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn opening_a_store_says_a_deployments_plain_http() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::Environment::var_name(), "production");
            let logs = nest_rs_testing::LogCapture::install();
            let config = StorageConfig {
                url: "s3://bucket".to_owned(),
                endpoint: Some("http://127.0.0.1:9".to_owned()),
                allow_http: nest_rs_config::Surface::Open,
                access_key: "key".into(),
                secret_key: "secret".into(),
                operation_timeout: Duration::from_millis(50),
                ..StorageConfig::default()
            };
            let answered = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(Store::open(&config));
            assert!(answered.is_err(), "nothing listens there");
            let line = logs.expect_one(crate::TARGET, crate::config::PLAINTEXT_OPEN);
            assert_eq!(line.level, "warn");
            Ok(())
        });
    }

    #[tokio::test]
    async fn a_store_rejecting_the_key_refuses_the_boot_at_once_naming_the_credentials() {
        let authority = nest_rs_testing::TestAuthority::new();
        let logs = nest_rs_testing::LogCapture::install();
        for (status, document) in rejected_keys() {
            let (endpoint, heard) =
                store_refusing(&authority, Arc::new(AtomicU16::new(status)), document).await;
            let refused = Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
                .await
                .expect_err("every real call would fail as this listing did");
            assert!(
                matches!(refused, StorageError::Refused { .. }),
                "{status}: {refused:?}"
            );
            let chain = nest_rs_core::error_message(&refused);
            for credential in ["ACCESS_KEY", "SECRET_KEY"] {
                assert!(
                    chain.contains(&nest_rs_config::var_name("storage", credential)),
                    "{chain}"
                );
            }
            assert!(!chain.contains(ECHOED_KEY), "{chain}");
            assert_eq!(heard.lines().len(), 1, "never retried: {:?}", heard.lines());
        }
        assert!(
            logs.find(crate::TARGET, REFUSED_TO_LIST).is_empty(),
            "a refusal is no warn and boot on: {:#?}",
            logs.events()
        );
    }

    #[tokio::test]
    async fn a_store_refusing_by_its_policy_boots_with_one_warn_naming_the_credentials() {
        let authority = nest_rs_testing::TestAuthority::new();
        let logs = nest_rs_testing::LogCapture::install();
        let answers = policy_refusals();
        for (status, document) in answers.clone() {
            let (endpoint, _) =
                store_refusing(&authority, Arc::new(AtomicU16::new(status)), document).await;
            Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
                .await
                .expect("a least-privilege policy is no reason to refuse the boot");
        }
        let warned = logs.find(crate::TARGET, REFUSED_TO_LIST);
        assert_eq!(
            warned.len(),
            answers.len(),
            "one per boot: {:#?}",
            logs.events()
        );
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
        let refused = Store::open(&reaching(&endpoint, "s3://bucket", ClientTls::default()))
            .await
            .expect_err("a certificate no trusted authority signed is not talked to");
        assert_eq!(heard.connections(), 1, "never retried");
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
        let refused = Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
            .await
            .expect_err("a store sending the bucket elsewhere is not proved");
        assert_eq!(
            (heard.connections(), heard.lines().len()),
            (1, 1),
            "never retried"
        );
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
        let refused = Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
            .await
            .expect_err("a store answering 400 is not proved");
        assert_eq!(
            (heard.connections(), heard.lines().len()),
            (1, 1),
            "never retried"
        );
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
    async fn a_probe_reads_a_store_refusing_by_its_policy_as_answering_as_the_boot_does() {
        let authority = nest_rs_testing::TestAuthority::new();
        for (status, document) in policy_refusals() {
            let (endpoint, _) =
                store_refusing(&authority, Arc::new(AtomicU16::new(status)), document).await;
            unproved(&endpoint, &authority)
                .answers()
                .await
                .expect("a least-privilege policy is no reason to turn traffic away");
        }
    }

    #[tokio::test]
    async fn a_probe_reads_a_store_rejecting_the_key_as_the_boot_refuses_it() {
        let authority = nest_rs_testing::TestAuthority::new();
        for (status, document) in rejected_keys() {
            let (endpoint, heard) =
                store_refusing(&authority, Arc::new(AtomicU16::new(status)), document).await;
            let refused = unproved(&endpoint, &authority)
                .answers()
                .await
                .expect_err("every real call would fail as this listing did");
            assert!(
                matches!(refused, StorageError::Refused { .. }),
                "{status}: {refused:?}"
            );
            assert_eq!(heard.lines().len(), 1, "{:?}", heard.lines());
        }
    }

    /// object_store reads the answer's body again, so what it meets is what the
    /// store sent.
    #[tokio::test]
    async fn a_refusal_read_hands_its_document_on_as_it_was_sent() {
        let document = error_document("SignatureDoesNotMatch");
        let sent = http::Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(HttpResponseBody::from(document.clone()))
            .expect("a response");

        let (answered, exchange) = read(sent).await;

        assert_eq!(answered.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            answered.into_body().bytes().await.expect("its body"),
            document.as_bytes(),
        );
        assert_eq!(
            exchange.to_string(),
            "the store rejected the key: 403 Forbidden, SignatureDoesNotMatch",
        );
    }

    /// Past the bound a refusal reads as one that says nothing: a store naming
    /// the key's code past it boots on, and nothing it sends is held in memory.
    #[tokio::test]
    async fn a_refusal_past_its_bound_is_read_as_saying_nothing_of_the_key() {
        let authority = nest_rs_testing::TestAuthority::new();
        let padded = format!(
            "{}{}",
            " ".repeat(REFUSAL_DOCUMENT_BOUND),
            error_document("SignatureDoesNotMatch")
        );
        let (endpoint, _) = store_refusing(&authority, Arc::new(AtomicU16::new(403)), padded).await;
        Store::open(&reaching(&endpoint, "s3://bucket", authority.client_tls()))
            .await
            .expect("read as a refusal saying nothing of the key");
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
        for (status, said) in [(403, 1), (200, 1), (403, 2), (403, 2), (500, 2), (403, 3)] {
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
