//! The HTTP guard stage — each guard's `check_http` in front of a site's tail,
//! the [`Denial`] → HTTP response mapping, and what the stage attaches. The
//! site is driven by calling `.call(..)` directly (no live socket).

use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use nest_rs_core::{
    __private::{HandlerDeclaration, MetaLevels, ResolvedLayer, declare_handler},
    async_trait,
};
use nest_rs_core::{Handler, Layer, LayerSite, Posture, UnitContext};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::behind_guards;
use nest_rs_http::{BoxEndpoint, Endpoint, Request, Response, endpoint_fn};
use nest_rs_testing::LogCapture;

struct Mount;

static UPGRADE: Handler = declare_handler(HandlerDeclaration {
    unit: nest_rs_http::unit::REQUEST,
    host: TypeId::of::<Mount>(),
    host_name: "Mount",
    method: "",
    address: "/ws",
    posture: Posture::Gated,
    meta: MetaLevels::default,
});

/// A guard whose decision is fixed at construction: `None` allows, `Some(d)`
/// denies with `d`. Increments a shared counter each time it runs, so a test
/// can prove a chained guard executed (or was short-circuited).
struct DecisionGuard {
    denial: Option<Denial>,
    ran: Arc<AtomicU32>,
}

impl DecisionGuard {
    fn allow(ran: Arc<AtomicU32>) -> Self {
        Self { denial: None, ran }
    }
    fn deny(denial: Denial, ran: Arc<AtomicU32>) -> Self {
        Self {
            denial: Some(denial),
            ran,
        }
    }
}

impl Layer for DecisionGuard {}

#[async_trait]
impl Guard for DecisionGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        match &self.denial {
            None => Ok(()),
            Some(d) => Err(d.clone()),
        }
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

fn entry(guard: DecisionGuard) -> ResolvedLayer<dyn Guard> {
    ResolvedLayer {
        type_id: TypeId::of::<DecisionGuard>(),
        name: "DecisionGuard",
        source: LayerSite::Host,
        layer: Arc::new(guard),
    }
}

fn guarded(chain: Vec<ResolvedLayer<dyn Guard>>, inner: impl Endpoint) -> BoxEndpoint {
    behind_guards(&UPGRADE, chain, BoxEndpoint::new(inner))
        .expect("a site declaring no `#[meta]` composes")
}

/// The handler behind the stage, counting its runs into `ran`.
fn handler(ran: Arc<AtomicU32>) -> impl Endpoint {
    endpoint_fn(move |_| {
        ran.fetch_add(1, Ordering::SeqCst);
        async { Ok(Response::builder().body("ok")) }
    })
}

async fn call_guarded(guard: DecisionGuard) -> Response {
    guarded(vec![entry(guard)], handler(Arc::default()))
        .call(Request::builder().finish())
        .await
        .expect("a guard stage maps a denial to Ok(Response), never Err")
}

#[tokio::test]
async fn an_allowing_guard_lets_the_request_reach_the_handler() {
    let resp = call_guarded(DecisionGuard::allow(Arc::new(AtomicU32::new(0)))).await;
    assert_eq!(resp.status(), nest_rs_http::StatusCode::OK);
}

#[tokio::test]
async fn an_unauthorized_denial_renders_401() {
    let resp = call_guarded(DecisionGuard::deny(
        Denial::unauthorized("no token"),
        Arc::new(AtomicU32::new(0)),
    ))
    .await;
    assert_eq!(resp.status(), nest_rs_http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_forbidden_denial_renders_403() {
    let resp = call_guarded(DecisionGuard::deny(
        Denial::forbidden("not your row"),
        Arc::new(AtomicU32::new(0)),
    ))
    .await;
    assert_eq!(resp.status(), nest_rs_http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_rate_limit_denial_renders_429_with_retry_after() {
    let resp = call_guarded(DecisionGuard::deny(
        Denial::rate_limited(42, "slow down"),
        Arc::new(AtomicU32::new(0)),
    ))
    .await;
    assert_eq!(resp.status(), nest_rs_http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        resp.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("42"),
        "a rate-limit denial must carry Retry-After",
    );
}

#[tokio::test]
async fn a_denial_short_circuits_before_the_handler_runs() {
    // The handler increments a marker; a denying guard in front must keep it 0.
    let handler_ran = Arc::new(AtomicU32::new(0));
    let ep = guarded(
        vec![entry(DecisionGuard::deny(
            Denial::forbidden("nope"),
            Arc::new(AtomicU32::new(0)),
        ))],
        handler(handler_ran.clone()),
    );
    let resp = ep
        .call(Request::builder().finish())
        .await
        .expect("denial is Ok(Response)");
    assert_eq!(resp.status(), nest_rs_http::StatusCode::FORBIDDEN);
    assert_eq!(
        handler_ran.load(Ordering::SeqCst),
        0,
        "the handler must not run once a guard denies",
    );
}

#[tokio::test]
async fn a_denial_at_this_site_is_visible_at_warn() {
    // This stage is the WS upgrade's gate: its denial must log at warn, as its
    // per-message twin's does.
    let logs = LogCapture::install();
    let resp = call_guarded(DecisionGuard::deny(
        Denial::unauthorized("no ticket"),
        Arc::new(AtomicU32::new(0)),
    ))
    .await;
    assert_eq!(resp.status(), nest_rs_http::StatusCode::UNAUTHORIZED);

    let event = logs.expect_one("nest_rs::layers", "guard denied the request");
    assert_eq!(event.level, "warn", "a denial is a warn+ event");
    assert_eq!(
        event.field("status").as_deref(),
        Some("401"),
        "the denial's status rides as a structured field",
    );
    assert!(
        event.field("guard").is_some(),
        "the denying guard must be named — a bare denial log is the defect",
    );
}

#[tokio::test]
async fn an_allowed_request_logs_no_denial() {
    // The floor must not fire on the happy path, or the signal is worthless.
    let logs = LogCapture::install();
    let resp = call_guarded(DecisionGuard::allow(Arc::new(AtomicU32::new(0)))).await;
    assert_eq!(resp.status(), nest_rs_http::StatusCode::OK);
    logs.expect_none("nest_rs::layers", "guard denied the request");
}

#[tokio::test]
async fn every_guard_of_the_stage_runs_when_each_allows() {
    let first_ran = Arc::new(AtomicU32::new(0));
    let second_ran = Arc::new(AtomicU32::new(0));
    let ep = guarded(
        vec![
            entry(DecisionGuard::allow(first_ran.clone())),
            entry(DecisionGuard::allow(second_ran.clone())),
        ],
        handler(Arc::default()),
    );
    let resp = ep
        .call(Request::builder().finish())
        .await
        .expect("both allow");
    assert_eq!(resp.status(), nest_rs_http::StatusCode::OK);
    assert_eq!(first_ran.load(Ordering::SeqCst), 1, "the first guard ran");
    assert_eq!(second_ran.load(Ordering::SeqCst), 1, "the second guard ran");
}

#[tokio::test]
async fn a_denial_short_circuits_the_guards_listed_after_it() {
    let second_ran = Arc::new(AtomicU32::new(0));
    let ep = guarded(
        vec![
            entry(DecisionGuard::deny(
                Denial::unauthorized("stop"),
                Arc::new(AtomicU32::new(0)),
            )),
            entry(DecisionGuard::allow(second_ran.clone())),
        ],
        handler(Arc::default()),
    );
    let resp = ep
        .call(Request::builder().finish())
        .await
        .expect("denial is Ok(Response)");
    assert_eq!(resp.status(), nest_rs_http::StatusCode::UNAUTHORIZED);
    assert_eq!(
        second_ran.load(Ordering::SeqCst),
        0,
        "a guard listed after the denying one must not run",
    );
}

mod declaration {
    use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
    use nest_rs_guards::{Denial, Guard, GuardFor};
    use nest_rs_http::{HttpView, controller, routes};
    use nest_rs_testing::TestApp;

    /// Declares a success among what it refuses with.
    #[injectable]
    #[derive(Default)]
    struct DeclaresASuccess;

    impl Layer for DeclaresASuccess {}

    #[async_trait]
    impl Guard for DeclaresASuccess {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }

        fn denial_statuses(&self, _public: bool) -> &'static [u16] {
            &[200]
        }
    }

    impl GuardFor<HttpView> for DeclaresASuccess {}

    #[controller(path = "/declared")]
    struct DeclaredController;

    #[routes]
    impl DeclaredController {
        #[get("/")]
        #[public]
        #[use_guards(DeclaresASuccess)]
        async fn list(&self) -> &'static str {
            "[]"
        }
    }

    #[module(providers = [DeclaresASuccess, DeclaredController])]
    struct DeclaredApp;

    /// A declared success would stand in the route's document for what it
    /// answers.
    #[tokio::test]
    async fn a_guard_declaring_a_non_error_status_fails_the_boot() {
        let refused = TestApp::for_module::<DeclaredApp>()
            .await
            .err()
            .expect("a guard declaring `200` must fail the boot");
        let said = nest_rs_core::error_message(&*refused);
        assert!(said.contains("DeclaresASuccess"), "names the guard: {said}");
        assert!(said.contains("200"), "names the status: {said}");
        assert!(said.contains("400 to 599"), "names the range: {said}");
    }
}
