//! The HTTP guard stage: each guard's [`Guard::check_http`] on a request,
//! read through the edge's [`HttpView`], and a [`Denial`] rendered as the
//! RFC 9457 problem the edge answers.
//!
//! What the guards attach is ambient once the whole stage passed: a stage that
//! wraps the rest of the request installs it there; an in-band endpoint whose
//! request runs several stages, one after another, keeps it on the request
//! until it runs the rest ([`install_attached`]).

use std::future::Future;
use std::sync::Arc;

use nest_rs_core::__private::{
    Pending, ResolvedLayer, UnmaskedReply, carries_ambient, install_pending, new_unit_context,
    take_pending,
};
use nest_rs_core::{AmbientValue, Handler};
use nest_rs_guards::__private::{Refused, denial_of, failure_chain};
use nest_rs_guards::{Denial, Guard, RequiredScopes};
use poem::http::{HeaderValue, StatusCode, header};
use poem::{Error, IntoResponse, Request, Response};

use crate::endpoint::{BoxEndpoint, Endpoint};
use crate::poem_bridge::{request_from_poem, request_to_poem};
use crate::problem::ProblemDetails;
use crate::shaper::GateFn;
use crate::view::HttpView;

/// What a request's guard stages attached so far, held on the request between a
/// stage and the endpoint that runs the rest of it.
#[derive(Clone, Default)]
struct Attached(Pending);

/// Run each of `guards`' [`check_http`](Guard::check_http), in order, on
/// `request` as the unit `handler` declares, then each of the route's class
/// `gates` on the same unit: the request back with what the stage attached, or
/// the first refusal.
///
/// The stage runs under what an earlier stage of the same request attached,
/// so a guard reads its predecessors' attachments through
/// `UnitContext::ambient` whichever stage they ran in.
async fn run_stage<'g>(
    handler: &'static Handler,
    guards: impl IntoIterator<Item = (&'static str, &'g dyn Guard)>,
    gates: &[GateFn],
    request: crate::Request,
) -> (crate::Request, Result<Pending, Refused>) {
    let earlier = pending_in(request.extensions());
    let mut view = HttpView::new(request);
    let checked = install_pending(earlier, async {
        let mut cx = new_unit_context(handler, &mut view, None, None);
        for (name, guard) in guards {
            guard
                .check_http(&mut cx)
                .await
                .map_err(|denial| Refused::by(name, guard, denial))?;
        }
        for gate in gates {
            gate(&mut cx).map_err(|refused| Refused::Filed(denial_of(&refused)))?;
        }
        Ok(take_pending(&mut cx))
    })
    .await;
    (view.into_request(), checked)
}

/// [`run_stage`] on a request poem hands over, given back as poem's.
async fn run_poem_stage<'g>(
    handler: &'static Handler,
    guards: impl IntoIterator<Item = (&'static str, &'g dyn Guard)>,
    gates: &[GateFn],
    request: Request,
) -> (Request, Result<Pending, Refused>) {
    let (request, checked) = run_stage(handler, guards, gates, request_from_poem(request)).await;
    (request_to_poem(request), checked)
}

/// A refusal's answer, filing first a guard's denial nobody filed.
fn answer(refused: Refused) -> Response {
    match refused {
        Refused::Guard(guard, denial) => deny_http(guard, denial),
        Refused::Filed(denial) => denial_to_http_response(denial),
    }
}

/// The denial a refusal carries, filing first a guard's nobody filed — the
/// in-band stage renders it as its edge does.
fn into_denial(refused: Refused) -> Denial {
    match refused {
        Refused::Guard(guard, denial) => {
            file_denial(guard, &denial);
            denial
        }
        Refused::Filed(denial) => denial,
    }
}

/// The guards of a composed chain, as the stage runs them: on the erased guard,
/// since the `Guard for Arc<T>` blanket would nest a second boxed future per
/// check.
fn erased(chain: &[ResolvedLayer<dyn Guard>]) -> impl Iterator<Item = (&'static str, &dyn Guard)> {
    chain.iter().map(|entry| (entry.name, entry.layer.as_ref()))
}

/// Run `guards`' HTTP entry on `req`, the unit `handler` declares, keeping what
/// they attach on the request for [`install_attached`] — the stage an in-band
/// endpoint runs before the rest of its request.
///
/// A refusal is filed here, as on a route, and comes back as raised: the
/// endpoint renders it as its edge does.
pub async fn check_request<'g>(
    handler: &'static Handler,
    guards: impl IntoIterator<Item = (&'static str, &'g dyn Guard)>,
    req: &mut crate::Request,
) -> Result<(), Denial> {
    let taken = std::mem::replace(req, crate::Request::builder().finish());
    let (request, checked) = run_stage(handler, guards, &[], taken).await;
    *req = request;
    let pending = checked.map_err(into_denial)?;
    let attached = req.extensions_mut();
    match attached.get_mut::<Attached>() {
        Some(Attached(earlier)) => earlier.merge(pending),
        None => {
            attached.insert(Attached(pending));
        }
    }
    Ok(())
}

/// [`check_request`] over a composed chain; an empty one leaves `req` as it is.
pub async fn check_request_chain(
    handler: &'static Handler,
    chain: &[ResolvedLayer<dyn Guard>],
    req: &mut crate::Request,
) -> Result<(), Denial> {
    if chain.is_empty() {
        return Ok(());
    }
    check_request(handler, erased(chain), req).await
}

/// Run `fut` under what `req`'s guard stages attached.
pub fn install_attached<F: Future>(
    req: &crate::Request,
    fut: F,
) -> impl Future<Output = F::Output> {
    install_pending(attached_pending(req), fut)
}

/// What `req`'s guard stages attached so far, to install around what runs
/// after them — later stages, the endpoint, work that leaves the request's task
/// (an MCP operation).
pub fn attached_pending(req: &crate::Request) -> Pending {
    pending_in(req.extensions())
}

/// What the guard stages recorded in `extensions` attached so far.
fn pending_in(extensions: &http::Extensions) -> Pending {
    extensions
        .get::<Attached>()
        .map(|attached| attached.0.clone())
        .unwrap_or_default()
}

/// The `T` one of `req`'s guard stages attached.
pub fn attached<T: AmbientValue>(req: &crate::Request) -> Option<Arc<T>> {
    req.extensions().get::<Attached>()?.0.get::<T>()
}

/// Run `chain` on `req` as the unit `handler` declares, then the route's class
/// `gates` on the same unit, then `rest` under what the guards attached — or
/// the refusal's answer, a guard's structural `warn` filed.
///
/// `unmasked` says no response mask shapes what `rest` answers, so the reads
/// a mask would scope are withheld from it ([`UnmaskedReply`]).
pub(crate) async fn guard_then<F, Fut>(
    handler: &'static Handler,
    chain: &[ResolvedLayer<dyn Guard>],
    gates: &[GateFn],
    req: Request,
    rest: F,
    unmasked: bool,
) -> poem::Result<Response>
where
    F: FnOnce(Request) -> Fut,
    Fut: Future<Output = poem::Result<Response>>,
{
    // Nothing to run: the request stays poem's, and nothing is attached.
    let (req, pending) = if chain.is_empty() && gates.is_empty() {
        (req, Pending::default())
    } else {
        match run_poem_stage(handler, erased(chain), gates, req).await {
            (req, Ok(pending)) => (req, pending),
            (_, Err(refused)) => return Ok(answer(refused)),
        }
    };
    install_pending(marked(pending, unmasked), rest(req)).await
}

/// `pending`, marked to withhold the reads a response mask would scope when
/// `unmasked`: with nothing attached here and nothing carried in, the route
/// reads none anyway, and nothing is marked.
fn marked(mut pending: Pending, unmasked: bool) -> Pending {
    if unmasked && (!pending.is_empty() || carries_ambient()) {
        pending.attach(Arc::new(UnmaskedReply));
    }
    pending
}

/// An endpoint behind a guard stage — a self-mount's request, a gateway's
/// upgrade: the chain runs as the unit `handler` declares, then the endpoint
/// under what it attached.
pub struct GuardedEndpoint {
    handler: &'static Handler,
    chain: Vec<ResolvedLayer<dyn Guard>>,
    inner: BoxEndpoint,
}

impl GuardedEndpoint {
    /// `inner` behind `chain`, run as the unit `handler` declares.
    pub fn new(
        handler: &'static Handler,
        chain: Vec<ResolvedLayer<dyn Guard>>,
        inner: BoxEndpoint,
    ) -> Self {
        Self {
            handler,
            chain,
            inner,
        }
    }
}

impl Endpoint for GuardedEndpoint {
    async fn call(&self, req: crate::Request) -> crate::Result<crate::Response> {
        let (req, pending) = match run_stage(self.handler, erased(&self.chain), &[], req).await {
            (req, Ok(pending)) => (req, pending),
            (_, Err(refused)) => {
                return Ok(crate::poem_bridge::response_from_poem(answer(refused)));
            }
        };
        install_pending(marked(pending, false), self.inner.call(req)).await
    }
}

/// A denial on an HTTP site, filed, then its answer.
pub fn deny_http(guard: &'static str, denial: Denial) -> Response {
    file_denial(guard, &denial);
    denial_to_http_response(denial)
}

/// The one `warn` that keeps every denial of an HTTP request visible whatever
/// the guard logged, carrying the chain of what failed when the denial keeps
/// one.
fn file_denial(guard: &'static str, denial: &Denial) {
    tracing::warn!(
        target: nest_rs_core::target::LAYERS,
        guard,
        status = denial.http_status(),
        error = failure_chain(denial),
        "guard denied the request",
    );
}

/// A [`Denial`] as the HTTP response the edge answers: the RFC 9457
/// `application/problem+json` envelope, a `4xx` reason as its `detail`, a
/// `5xx` keeping only the generic title so no internal text leaks.
pub fn denial_to_http_response(denial: Denial) -> Response {
    let mut response = problem_response(&denial);
    // The scopes ride as an extension: the challenge also names the RFC 9728
    // metadata document, which only the oauth-resource interceptor knows.
    if let Some(required) = required_scopes(&denial) {
        response.extensions_mut().insert(required);
    }
    response
}

/// The problem+json envelope alone — everything [`denial_to_http_response`]
/// builds except the scope evidence, which the `Err` path must attach to the
/// [`Error`] instead of to this response.
fn problem_response(denial: &Denial) -> Response {
    let status =
        StatusCode::from_u16(denial.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut problem = ProblemDetails::from_status(status);
    if status.is_client_error() {
        problem = problem.with_detail(denial.message().to_owned());
    }
    let mut response = problem.into_response();
    // RFC 6750 §3/§3.1: a rejected credential names its code. Not gated on the
    // status: `insufficient_scope` lives at `403`, and `bearer_error` is already
    // `None` wherever no challenge is due.
    if let Some(code) = denial.bearer_error() {
        crate::challenge::stamp_bearer_error(&mut response, code);
    }
    // RFC 6585 §4 for a rate limit, RFC 9110 §15.6.4 for an unavailable
    // dependency: the wait, in delay-seconds, whenever it is known.
    if let Some(retry_after_secs) = denial.retry_after_secs() {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
    }
    response
}

/// The evidence a scope denial carries to the edge, or `None` when there is
/// none to carry — an empty set says no more than a bare `403`, so no transport
/// emits a challenge for it.
fn required_scopes(denial: &Denial) -> Option<RequiredScopes> {
    let required = denial.required_scopes();
    (!required.is_empty()).then(|| RequiredScopes::new(required.to_vec()))
}

/// A [`Denial`] as a poem [`Error`] — the `Err` path's counterpart to
/// [`denial_to_http_response`], for the sites that must reject rather than
/// return (an extractor, the MCP endpoint).
///
/// Not `Error::from_response(denial_to_http_response(d))`: poem's
/// `Error::into_response` overwrites the response's extensions with the error's
/// own, dropping the scope evidence; `set_data` is the channel that survives.
pub fn denial_to_http_error(denial: Denial) -> Error {
    let mut error = Error::from_response(problem_response(&denial));
    if let Some(required) = required_scopes(&denial) {
        error.set_data(required);
    }
    error
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler, meta_value};
    use nest_rs_core::{Layer, LayerSite, Posture, UnitContext};
    use nest_rs_guards::async_trait;

    use super::*;

    #[tokio::test]
    async fn unauthorized_denial_renders_problem_json() {
        let resp = denial_to_http_response(Denial::unauthorized("missing bearer token"));
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["status"], 401);
        assert_eq!(json["title"], "Unauthorized");
        assert_eq!(json["detail"], "missing bearer token");
    }

    #[tokio::test]
    async fn rate_limited_denial_keeps_retry_after_on_problem_json() {
        let resp = denial_to_http_response(Denial::rate_limited(30, "slow down"));
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"30".as_slice()),
        );
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
        );
    }

    #[tokio::test]
    async fn insufficient_scope_carries_the_required_scopes_to_the_edge() {
        // The transport edge is what turns these into the RFC 6750 challenge;
        // losing them here would leave a client a bare 403 it cannot act on.
        let resp = denial_to_http_response(Denial::insufficient_scope(
            ["posts:write"],
            "this token may not write posts",
        ));
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.extensions()
                .get::<RequiredScopes>()
                .map(RequiredScopes::as_slice),
            Some(["posts:write".to_owned()].as_slice()),
        );
        assert_eq!(
            Denial::insufficient_scope(["posts:write"], "narrow").bearer_error(),
            Some(crate::challenge::INSUFFICIENT_SCOPE),
            "the denial names the code the challenge writes",
        );
    }

    #[tokio::test]
    async fn the_err_path_carries_the_scopes_through_poems_extension_overwrite() {
        // `Error::from_response` alone loses them: `into_response` replaces the
        // response's extensions wholesale.
        let error = denial_to_http_error(Denial::insufficient_scope(
            ["posts:write"],
            "this token may not write posts",
        ));
        let resp = error.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.extensions()
                .get::<RequiredScopes>()
                .map(RequiredScopes::as_slice),
            Some(["posts:write".to_owned()].as_slice()),
            "a denial that travels the `Err` path must reach the edge intact",
        );
    }

    #[tokio::test]
    async fn a_scope_denial_naming_nothing_is_an_ordinary_forbidden() {
        // A deployment may refuse without naming its internals; `scope=""`
        // would be a malformed challenge, so the edge must see no marker.
        let resp = denial_to_http_response(Denial::insufficient_scope(
            Vec::<String>::new(),
            "forbidden",
        ));
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(resp.extensions().get::<RequiredScopes>().is_none());
    }

    #[tokio::test]
    async fn internal_denial_is_a_500_problem_without_leaking_detail() {
        let resp = denial_to_http_response(Denial::internal("panic: secret config missing"));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            !text.contains("secret config"),
            "a 5xx denial must not leak internal detail: {text}",
        );
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(json.get("detail").is_none(), "no detail on a 500 denial");
    }

    /// The HTTP half: `503` (RFC 9110 §15.6.4), `Retry-After` when known, and
    /// the generic title of a 5xx rather than the reason.
    #[tokio::test]
    async fn an_unavailable_dependency_is_a_503_with_its_known_wait() {
        let resp =
            denial_to_http_response(Denial::unavailable(Some(7), "authentication unavailable"));
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"7".as_slice()),
        );
        let unknown =
            denial_to_http_response(Denial::unavailable(None, "authentication unavailable"));
        assert_eq!(unknown.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(unknown.headers().get(header::RETRY_AFTER).is_none());
    }

    /// The HTTP half of a rate limit, pinned beside the in-band edges' wait:
    /// one number, one unit.
    #[tokio::test]
    async fn the_http_edge_reports_the_wait_as_a_header() {
        let resp = denial_to_http_response(Denial::rate_limited(42, "too many requests"));
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"42".as_slice()),
        );
    }

    #[derive(Debug, PartialEq)]
    struct Tenant(&'static str);

    impl AmbientValue for Tenant {}

    #[derive(Clone, Debug, PartialEq)]
    struct Throttle(u32);

    struct Controller;

    fn declared() -> MetaLevels {
        MetaLevels {
            host: vec![meta_value(Throttle(100))],
            method: vec![meta_value(Throttle(10))],
        }
    }

    static ROUTE: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::REQUEST,
        host: TypeId::of::<Controller>(),
        host_name: "Controller",
        method: "route",
        address: "/x",
        posture: Posture::Public,
        meta: declared,
    });

    /// Attaches the tenant its request names, and inserts its own `Throttle`
    /// where a guard could once shadow the declared one.
    struct TenantGuard;

    impl Layer for TenantGuard {}

    #[async_trait]
    impl Guard for TenantGuard {
        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let request = HttpView::of(cx)?.request_mut();
            let tenant = match request.header("x-tenant") {
                Some("acme") => "acme",
                _ => return Err(Denial::forbidden("an unknown tenant")),
            };
            request.extensions_mut().insert(Throttle(1));
            cx.attach(Tenant(tenant));
            Ok(())
        }
    }

    /// Reads, in a later stage of the same request, what the first attached
    /// and what the site declared.
    struct Witness;

    impl Layer for Witness {}

    #[async_trait]
    impl Guard for Witness {
        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let tenant = cx.ambient::<Tenant>().map(|tenant| tenant.0);
            let throttle = cx.reflector().get::<Throttle>().map(|throttle| throttle.0);
            match (tenant, throttle, cx.reflector().is_public()) {
                (Some("acme"), Some(10), true) => Ok(()),
                seen => Err(Denial::internal(format!("saw {seen:?}"))),
            }
        }
    }

    fn chain<G: Guard>(guard: G) -> Vec<ResolvedLayer<dyn Guard>> {
        vec![ResolvedLayer {
            type_id: TypeId::of::<G>(),
            name: std::any::type_name::<G>(),
            source: LayerSite::Global,
            layer: Arc::new(guard),
        }]
    }

    fn tenant_request() -> Request {
        Request::builder().header("x-tenant", "acme").finish()
    }

    #[tokio::test]
    async fn a_later_stage_reads_what_an_earlier_one_attached_and_never_a_shadow() {
        let mut req = request_from_poem(tenant_request());
        check_request_chain(&ROUTE, &chain(TenantGuard), &mut req)
            .await
            .expect("the tenant is known");
        check_request_chain(&ROUTE, &chain(Witness), &mut req)
            .await
            .expect("the second stage reads the first one's tenant and the declared throttle");

        let seen = install_attached(&req, async { nest_rs_core::ambient::<Tenant>() }).await;
        assert_eq!(seen.as_deref(), Some(&Tenant("acme")));
        assert_eq!(attached::<Tenant>(&req).as_deref(), Some(&Tenant("acme")));
        assert!(
            nest_rs_core::ambient::<Tenant>().is_none(),
            "nothing is ambient outside what the endpoint runs under it",
        );
    }

    #[tokio::test]
    async fn a_refused_stage_attaches_nothing() {
        let logs = nest_rs_testing::LogCapture::install();
        let mut req = request_from_poem(Request::builder().header("x-tenant", "globex").finish());
        let denial = check_request_chain(&ROUTE, &chain(TenantGuard), &mut req)
            .await
            .expect_err("an unknown tenant is refused");
        assert_eq!(denial.http_status(), 403);
        assert!(attached::<Tenant>(&req).is_none());
        let event = logs.expect_one("nest_rs::layers", "guard denied the request");
        assert!(
            event
                .field("guard")
                .is_some_and(|guard| guard.ends_with("TenantGuard")),
            "the line names the guard that refused, got {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("error"),
            None,
            "a forbidden caller is no failure"
        );
    }

    #[tokio::test]
    async fn a_guarded_endpoint_runs_its_inner_endpoint_under_what_the_stage_attached() {
        let inner = poem::endpoint::make(|_req: Request| async {
            nest_rs_core::ambient::<Tenant>()
                .map(|tenant| tenant.0)
                .unwrap_or("none")
        });
        let guarded = GuardedEndpoint::new(
            &ROUTE,
            chain(TenantGuard),
            BoxEndpoint::new(crate::poem_bridge::from_poem(inner)),
        );

        let admitted = guarded
            .call(request_from_poem(tenant_request()))
            .await
            .expect("answers");
        assert_eq!(
            admitted.into_body().into_string().await.expect("a body"),
            "acme"
        );

        let logs = nest_rs_testing::LogCapture::install();
        let refused = guarded
            .call(crate::Request::builder().finish())
            .await
            .expect("answers");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let event = logs.expect_one("nest_rs::layers", "guard denied the request");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("status").as_deref(), Some("403"));
    }

    /// What a store refusing the connection reports, the cause a guard keeps.
    #[derive(Debug)]
    struct ConnectionRefused;

    impl std::fmt::Display for ConnectionRefused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the identity store refused the connection")
        }
    }

    impl std::error::Error for ConnectionRefused {}

    /// Cannot evaluate the request: its identity store is down.
    struct StoreDown;

    impl Layer for StoreDown {}

    #[async_trait]
    impl Guard for StoreDown {
        async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Err(Denial::unavailable_because(
                None,
                "the identity store did not answer",
                ConnectionRefused,
            ))
        }
    }

    const STORE_DOWN_CHAIN: &str =
        "the identity store did not answer: the identity store refused the connection";

    #[tokio::test]
    async fn a_route_denial_keeping_its_cause_files_the_chain_and_answers_without_it() {
        let logs = nest_rs_testing::LogCapture::install();
        let inner = poem::endpoint::make(|_req: Request| async { "reached" });
        let guarded = GuardedEndpoint::new(
            &ROUTE,
            chain(StoreDown),
            BoxEndpoint::new(crate::poem_bridge::from_poem(inner)),
        );

        let refused = guarded
            .call(crate::Request::builder().finish())
            .await
            .expect("answers");
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = refused.into_body().into_string().await.expect("a body");
        assert!(
            !body.contains("refused"),
            "the cause stays on the server: {body}"
        );
        let event = logs.expect_one("nest_rs::layers", "guard denied the request");
        assert_eq!(event.field("status").as_deref(), Some("503"));
        assert_eq!(event.field("error").as_deref(), Some(STORE_DOWN_CHAIN));
    }

    #[tokio::test]
    async fn an_in_band_request_stage_files_a_denials_chain() {
        let logs = nest_rs_testing::LogCapture::install();
        let denial = check_request_chain(
            &ROUTE,
            &chain(StoreDown),
            &mut crate::Request::builder().finish(),
        )
        .await
        .expect_err("the store is down");
        assert_eq!(denial.http_status(), 503);
        let event = logs.expect_one("nest_rs::layers", "guard denied the request");
        assert_eq!(event.field("error").as_deref(), Some(STORE_DOWN_CHAIN));
    }

    /// Admits every request and attaches nothing.
    struct Admitting;

    impl Layer for Admitting {}

    #[async_trait]
    impl Guard for Admitting {
        async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    /// Whether what an unmasked route runs after `chain` sees the marker.
    async fn withheld(chain: &[ResolvedLayer<dyn Guard>], req: Request) -> bool {
        let rest = |_req| async {
            let marked = nest_rs_core::ambient::<UnmaskedReply>().is_some();
            Ok(Response::builder().body(if marked { "withheld" } else { "bare" }))
        };
        let answer = guard_then(&ROUTE, chain, &[], req, rest, true)
            .await
            .expect("answers");
        answer.into_body().into_string().await.expect("a body") == "withheld"
    }

    #[tokio::test]
    async fn an_unmasked_route_marks_only_a_unit_carrying_something_to_withhold() {
        assert!(!withheld(&[], Request::default()).await, "no guard ran");
        assert!(
            !withheld(&chain(Admitting), Request::default()).await,
            "its guard attached nothing",
        );
        assert!(
            withheld(&chain(TenantGuard), tenant_request()).await,
            "what its guard attached is withheld",
        );
        let carried = nest_rs_core::with_ambient(
            Arc::new(Tenant("acme")),
            withheld(&chain(Admitting), Request::default()),
        )
        .await;
        assert!(carried, "what the unit carried in is withheld too");
    }
}
