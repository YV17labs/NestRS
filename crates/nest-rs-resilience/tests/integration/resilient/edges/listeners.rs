//! A decorated listener of a `#[listeners]` block, attempted again within the
//! one call an emit makes, and refused: its unit filed failed, the emit going
//! on.

use std::sync::Arc;

use nest_rs_core::operation_log::{ERROR, OK, TARGET as OPERATIONS};
use nest_rs_core::{injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::{HeadlessApp, LogCapture};

use super::{Attempts, Dependency, Refusing, UpstreamDown, refusing, retrying_once};

/// A parcel left the depot, which the listener tells the carrier.
#[derive(Clone)]
struct Shipped;

#[injectable]
struct ShippedListener {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    attempts: Arc<Attempts>,
}

#[listeners]
impl ShippedListener {
    #[on_event]
    #[resilient(upstream, idempotent)]
    async fn on_shipped(&self, _event: Shipped) -> Result<(), ResilienceError<UpstreamDown>> {
        if self.attempts.first() {
            return Err(UpstreamDown);
        }
        Ok(())
    }
}

#[module(
    imports = [EventsModule, ResilienceModule::for_root(retrying_once())],
    providers = [ShippedListener, Attempts],
)]
struct DecoratedEventsModule;

#[tokio::test(start_paused = true)]
async fn a_listener_failing_once_answers_on_its_retry_within_one_emit() {
    let logs = LogCapture::install();
    let app = HeadlessApp::builder()
        .module::<DecoratedEventsModule>()
        .build_headless()
        .await
        .expect("a decorated listener boots");
    let bus = app
        .container()
        .get::<EventBus>()
        .expect("the bus is provided");
    bus.emit(Shipped).await;

    let attempts = app.container().get::<Attempts>().expect("provided");
    assert_eq!(attempts.made(), 2, "the failed attempt and its retry");
    let ran = logs.expect_one(OPERATIONS, nest_rs_events::unit::DISPATCH.name());
    assert_eq!(ran.field("outcome").as_deref(), Some(OK));
}

/// Whom a returned parcel goes back to, which no line may say.
const RECIPIENT: &str = "ada.lovelace@example.com";

/// A parcel came back, which two listeners tell the dependency.
#[derive(Clone)]
struct Returned {
    recipient: &'static str,
}

#[injectable]
struct ReturnsListener {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    #[inject]
    dependency: Arc<Dependency>,
}

#[listeners]
impl ReturnsListener {
    #[on_event]
    #[resilient(upstream, timeout = "100ms")]
    async fn refund(&self, event: Returned) -> Result<(), ResilienceError<UpstreamDown>> {
        assert_eq!(event.recipient, RECIPIENT);
        self.dependency.call().await
    }

    #[on_event]
    #[resilient(upstream, timeout = "100ms")]
    async fn restock(&self, event: Returned) -> Result<(), ResilienceError<UpstreamDown>> {
        assert_eq!(event.recipient, RECIPIENT);
        self.dependency.call().await
    }
}

#[module(
    imports = [EventsModule, ResilienceModule::for_root(refusing())],
    providers = [ReturnsListener, Dependency],
)]
struct RefusedEventsModule;

/// No client reads a listener's `503`: its refusal fails its own unit,
/// filed as any failure of a listener, the listener after it is still
/// called — refused in its turn here, so whichever the bus calls first —
/// and the emit returns to its caller.
#[tokio::test(start_paused = true)]
async fn a_listener_its_policy_refuses_fails_its_unit_and_the_emit_goes_on() {
    for refusing in Refusing::EACH {
        let logs = LogCapture::install();
        let app = HeadlessApp::builder()
            .module::<RefusedEventsModule>()
            .build_headless()
            .await
            .expect("a decorated listener boots");
        let listener = app.container().get::<ReturnsListener>().expect("provided");
        let bus = app
            .container()
            .get::<EventBus>()
            .expect("the bus is provided");
        let refused = refusing.arrange(app.container()).await;
        let (upstream, dependency) = (&refused.upstream, &refused.dependency);
        let mut events = upstream.events();

        bus.emit(Returned {
            recipient: RECIPIENT,
        })
        .await;
        let told = crate::told(&mut events);
        assert_eq!(dependency.called(), 2 * refusing.sent());

        let mounted = logs.find(nest_rs_events::TARGET, "mounted");
        let failed = logs.find(
            nest_rs_events::TARGET,
            "event listener failed; unavailable — dispatch continues with the next listener",
        );
        let ran = logs.find(OPERATIONS, nest_rs_events::unit::DISPATCH.name());
        let chain = refusing.chain(
            listener
                .refund(Returned {
                    recipient: RECIPIENT,
                })
                .await,
        );
        for method in ["refund", "restock"] {
            let named = |lines: &[nest_rs_testing::CapturedEvent]| {
                let mut named = lines
                    .iter()
                    .filter(|line| line.field("method").as_deref() == Some(method));
                let line = named.next().cloned();
                assert!(
                    named.next().is_none(),
                    "one line for `{method}`: {lines:#?}"
                );
                line.unwrap_or_else(|| panic!("a line for `{method}`: {lines:#?}"))
            };
            let (mounted, failed, ran) = (named(&mounted), named(&failed), named(&ran));
            assert_eq!(
                failed.level, "warn",
                "a `503`, as every edge files it: {failed:?}"
            );
            assert_eq!(
                failed.field("error").as_deref(),
                Some(&*chain),
                "{failed:?}"
            );
            assert_eq!(ran.field("outcome").as_deref(), Some(ERROR), "{ran:?}");
            for line in [&failed, &ran] {
                assert_eq!(line.field("host"), mounted.field("host"), "{line:?}");
                assert_eq!(line.field("event"), mounted.field("event"), "{line:?}");
            }
            assert_eq!(mounted.field("host").as_deref(), Some("ReturnsListener"));
            assert_eq!(mounted.field("event").as_deref(), Some("Returned"));
            refusing.told(&told, "ReturnsListener", method);
        }
        assert_eq!(
            failed.len(),
            2,
            "each listener's refusal, once: {failed:#?}"
        );
        assert!(logs.events_at(tracing::Level::ERROR).is_empty());
        assert!(
            !logs.quotes(RECIPIENT),
            "no line says what the event carried"
        );
    }
}
