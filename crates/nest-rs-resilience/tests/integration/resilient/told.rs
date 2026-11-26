//! What the dependency answered is told where an operator reads it: the chain
//! of the error an attempt answered rides the retry's line and the breaker's
//! opening, so a run ending on the breaker's refusal still leaves its cause.

use super::*;

#[derive(Debug, thiserror::Error)]
#[error("the label printer refused the job")]
struct PrinterDown(#[source] std::io::Error);

const CHAIN: &str = "the label printer refused the job: connection reset by peer";

#[injectable]
struct LabelsService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
}

impl LabelsService {
    #[resilient(courier, idempotent)]
    async fn print(&self) -> Result<(), ResilienceError<PrinterDown>> {
        Err(PrinterDown(std::io::Error::other(
            "connection reset by peer",
        )))
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        retries: Some(2),
        breaker_window: Some(20),
        breaker_failures: 10,
        ..ResilienceConfig::default()
    }))],
    providers = [LabelsService],
)]
struct LabelsModule;

fn labels(app: &HeadlessApp) -> Arc<LabelsService> {
    app.container()
        .get::<LabelsService>()
        .expect("the module provides it")
}

#[tokio::test(start_paused = true)]
async fn a_retrys_line_carries_the_chain_of_the_error_its_attempt_answered() {
    let app = boot::<LabelsModule>().await;
    let labels = labels(&app);
    let logs = LogCapture::install();

    let answered = labels.print().await;
    assert!(
        matches!(answered, Err(ResilienceError::Failed(_))),
        "{answered:?}"
    );
    let retried = logs.find(
        nest_rs_resilience::TARGET,
        "attempt failed; retrying the call",
    );
    assert_eq!(retried.len(), 2, "{retried:#?}");
    for line in &retried {
        assert_eq!(line.field("error").as_deref(), Some(CHAIN), "{line:?}");
    }
}

/// Fifty idempotent callers of a dependency that is down: the breaker
/// opens on the first ten failures, and every caller whose retry it then
/// refuses answers `Open`. The opening's `warn` carries the chain of the
/// failure that opened it, the one line a production filter keeps.
#[tokio::test(start_paused = true)]
async fn the_breakers_opening_carries_the_chain_of_the_failure_that_opened_it() {
    let app = boot::<LabelsModule>().await;
    let labels = labels(&app);
    let logs = LogCapture::install();

    let callers: Vec<_> = (0..50)
        .map(|_| {
            let labels = Arc::clone(&labels);
            tokio::spawn(async move { labels.print().await })
        })
        .collect();
    let mut refused = 0;
    for caller in callers {
        match caller.await.expect("no caller panics") {
            Err(ResilienceError::Open { .. }) => refused += 1,
            Err(ResilienceError::Failed(_)) => {}
            other => panic!("a down dependency fails or is refused: {other:?}"),
        }
    }
    assert!(refused > 0, "the breaker opened under its callers");

    let opened = logs
        .find(nest_rs_resilience::TARGET, "circuit breaker changed state")
        .into_iter()
        .find(|line| line.field("state").as_deref() == Some("open"))
        .expect("the breaker's opening is filed");
    assert_eq!(opened.field("error").as_deref(), Some(CHAIN), "{opened:?}");
}

/// What the tracker's URL carries, in its path and in its query.
const SENT: &str = "s3cr3t";

/// An app's error holding reqwest's as it came, its URL not taken out.
#[derive(Debug, thiserror::Error)]
#[error("the tracker could not be reached")]
struct TrackerDown(#[from] reqwest::Error);

#[injectable]
struct TrackingService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
}

impl TrackingService {
    /// A call to a port nothing listens on.
    #[resilient(courier, idempotent)]
    async fn track(&self) -> Result<String, ResilienceError<TrackerDown>> {
        let url = format!("https://127.0.0.1:1/parcels/{SENT}?token={SENT}");
        Ok(reqwest::get(url).await?.text().await?)
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        retries: Some(1),
        backoff_first: Duration::from_millis(1),
        backoff_ceiling: Duration::from_millis(2),
        breaker_window: Some(2),
        breaker_failures: 2,
        ..ResilienceConfig::default()
    }))],
    providers = [TrackingService],
)]
struct TrackingModule;

/// reqwest's sentence quotes the URL a request was sent to, and an app's
/// error may hold reqwest's as it came: the retry's line and the breaker's
/// opening say its chain all the same, without that URL.
#[tokio::test]
async fn a_reqwest_error_an_app_wrapped_with_its_url_is_told_without_it() {
    let app = boot::<TrackingModule>().await;
    let tracking = app
        .container()
        .get::<TrackingService>()
        .expect("the module provides it");
    let logs = LogCapture::install();

    let answered = tracking.track().await;
    let Err(ResilienceError::Failed(TrackerDown(failed))) = &answered else {
        panic!("both attempts are refused the connection: {answered:?}");
    };
    assert!(
        failed.to_string().contains(SENT),
        "the error holds its URL: {failed}"
    );

    let retried = logs.expect_one(
        nest_rs_resilience::TARGET,
        "attempt failed; retrying the call",
    );
    let opened = logs
        .find(nest_rs_resilience::TARGET, "circuit breaker changed state")
        .into_iter()
        .find(|line| line.field("state").as_deref() == Some("open"))
        .expect("the second failure opened the breaker");
    for line in [&retried, &opened] {
        let error = line.field("error").unwrap_or_default();
        assert!(
            error.starts_with("the tracker could not be reached: error sending request: "),
            "the app's sentence, reqwest's, then its causes: {line:?}"
        );
        for quoted in [SENT, "/parcels", "for url"] {
            assert!(!line.quotes(quoted), "`{quoted}` in {line:?}");
        }
    }
    assert_eq!(
        retried.field("error"),
        opened.field("error"),
        "one failure, said the same in both"
    );
}

/// An app's error handing reqwest's sentence on as its own: reqwest's
/// error is then no link of its chain, and no driver's `SafeLink` reads it.
#[derive(Debug, thiserror::Error)]
enum TrackError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[injectable]
struct HandingOnService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
}

impl HandingOnService {
    /// A call to a port nothing listens on.
    #[resilient(courier, idempotent)]
    async fn track(&self) -> Result<String, ResilienceError<TrackError>> {
        let url = format!("https://user:{SENT}@127.0.0.1:1/parcels/{SENT}?token={SENT}");
        Ok(reqwest::get(url).await?.text().await?)
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, ResilienceConfig {
        retries: Some(1),
        backoff_first: Duration::from_millis(1),
        backoff_ceiling: Duration::from_millis(2),
        breaker_window: Some(2),
        breaker_failures: 2,
        ..ResilienceConfig::default()
    }))],
    providers = [HandingOnService],
)]
struct HandingOnModule;

/// Whatever wrote the sentence, the retry's line and the breaker's opening
/// tell a URL by its origin alone: the dependency reached, never the path,
/// the query or the userinfo its request carried.
#[tokio::test]
async fn a_url_an_apps_error_hands_on_as_its_own_sentence_is_told_by_its_origin() {
    let app = boot::<HandingOnModule>().await;
    let tracking = app
        .container()
        .get::<HandingOnService>()
        .expect("the module provides it");
    let logs = LogCapture::install();

    let answered = tracking.track().await;
    let Err(ResilienceError::Failed(failed)) = &answered else {
        panic!("both attempts are refused the connection: {answered:?}");
    };
    assert!(
        nest_rs_core::error_message(failed).contains(SENT),
        "the error's chain quotes its URL: {failed}"
    );

    let retried = logs.expect_one(
        nest_rs_resilience::TARGET,
        "attempt failed; retrying the call",
    );
    let opened = logs
        .find(nest_rs_resilience::TARGET, "circuit breaker changed state")
        .into_iter()
        .find(|line| line.field("state").as_deref() == Some("open"))
        .expect("the second failure opened the breaker");
    for line in [&retried, &opened] {
        let error = line.field("error").unwrap_or_default();
        assert!(
            error.starts_with("error sending request for url (https://127.0.0.1:1): "),
            "reqwest's sentence, its URL cut to its origin, then its causes: {line:?}"
        );
        for quoted in [SENT, "/parcels", "token", "user"] {
            assert!(!line.quotes(quoted), "`{quoted}` in {line:?}");
        }
    }
}
