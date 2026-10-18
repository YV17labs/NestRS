//! The HTTP guard stage: each guard's [`Guard::check_http`] on a request,
//! read through the edge's [`HttpView`], then the site's class gates, as the
//! pipeline's [`GuardStage`]; and a [`Denial`] rendered as the RFC 9457
//! problem the edge answers.
//!
//! What the guards attach is ambient once the whole stage passed, for the rest
//! of the unit; a self-mount's stage also keeps it on the request for work the
//! mount runs off its task ([`attached_pending`]).

use std::sync::Arc;

use futures_util::future::BoxFuture;
use http::{HeaderValue, StatusCode, header};
use nest_rs_core::__private::{
    GuardStage, Pending, ResolvedLayer, UnmaskedReply, carries_ambient, install_pending,
    new_unit_context, pending_of, take_pending,
};
use nest_rs_core::{AmbientValue, Failure, Handler, UnitContext};
use nest_rs_guards::__private::{RefusalMark, Refused, denial_of, failure_chain};
use nest_rs_guards::{Denial, Guard, RequiredScopes};

use crate::error::HttpError;
use crate::problem::ProblemDetails;
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::shaper::GateFn;
use crate::view::HttpView;

/// What a request's guard stages attached so far, held on the request between a
/// stage and the work its endpoint runs off its task.
#[derive(Clone, Default)]
struct Attached(Pending);

/// Run each of `guards`' [`check_http`](Guard::check_http), in order, on the
/// unit `cx` describes, then each of the site's class `gates` on the same unit.
/// The first refusal stops the stage.
async fn run_guards(
    guards: &[ResolvedLayer<dyn Guard>],
    gates: &[GateFn],
    cx: &mut UnitContext<'_>,
) -> Result<(), Refused> {
    for entry in guards {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        let guard = entry.layer.as_ref();
        guard
            .check_http(cx)
            .await
            .map_err(|denial| Refused::by(entry.name, guard, denial))?;
    }
    for gate in gates {
        gate(cx).map_err(|refused| Refused::Filed(denial_of(&refused)))?;
    }
    Ok(())
}

/// The guard stage of an HTTP site: its guards, then its class gates, as the
/// pipeline runs a site's guard stage.
pub(crate) struct HttpGuardStage {
    guards: Vec<ResolvedLayer<dyn Guard>>,
    gates: Vec<GateFn>,
    kind: StageKind,
}

/// What a stage does beside its guards once they passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageKind {
    /// A route; `unmasked` when no response mask shapes its reply, so the
    /// reads a mask would scope are withheld from it ([`UnmaskedReply`]).
    Route {
        /// No response shaper masks the route's reply.
        unmasked: bool,
    },
    /// A self-mount's request: what the guards attached is also kept on the
    /// request, for the units the mount runs off its task.
    SelfMount,
}

impl HttpGuardStage {
    /// The stage running `guards`, then `gates`.
    pub(crate) fn new(
        guards: Vec<ResolvedLayer<dyn Guard>>,
        gates: Vec<GateFn>,
        kind: StageKind,
    ) -> Self {
        Self {
            guards,
            gates,
            kind,
        }
    }

    /// What the stage records once its guards passed.
    fn passed(&self, cx: &mut UnitContext<'_>) {
        match self.kind {
            StageKind::Route { unmasked: true } => {
                // With nothing attached here and nothing carried in, the route
                // reads none anyway, and nothing is marked.
                if !pending_of(cx).is_empty() || carries_ambient() {
                    cx.attach(UnmaskedReply);
                }
            }
            StageKind::Route { unmasked: false } => {}
            StageKind::SelfMount => {
                let mut pending = pending_of(cx).clone();
                // A unit the mount carries off its task marks the request's
                // unit when its own guards refuse it.
                if let Some(mark) = nest_rs_core::ambient::<RefusalMark>() {
                    pending.attach(mark);
                }
                if let Ok(view) = HttpView::of(cx) {
                    let extensions = view.request_mut().extensions_mut();
                    match extensions.get_mut::<Attached>() {
                        Some(Attached(earlier)) => earlier.merge(pending),
                        None => {
                            extensions.insert(Attached(pending));
                        }
                    }
                }
            }
        }
    }
}

impl GuardStage for HttpGuardStage {
    fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            match run_guards(&self.guards, &self.gates, cx).await {
                Ok(()) => {
                    self.passed(cx);
                    Ok(())
                }
                Err(refused) => Err(answer(cx, refused.into_denial(file_denial))),
            }
        })
    }
}

/// The stage of a gated self-mount carrying in-band units with no guard at its
/// request: every request is refused, and filed.
pub(crate) struct DeniedStage {
    path: String,
}

impl DeniedStage {
    /// Refuse every request to the self-mount at `path`.
    pub(crate) fn new(path: String) -> Self {
        Self { path }
    }
}

impl GuardStage for DeniedStage {
    fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>> {
        tracing::warn!(
            target: nest_rs_core::target::LAYERS,
            path = self.path.as_str(),
            "request denied: the endpoint runs no guard at its request",
        );
        let denied = answer(
            cx,
            Denial::unauthorized("no guard admits requests to this endpoint"),
        );
        Box::pin(std::future::ready(Err(denied)))
    }
}

/// The refused unit's answer: `denial` rendered into its view, and the unit
/// failed with it, answered, so no exception filter maps it.
pub(crate) fn answer(cx: &mut UnitContext<'_>, denial: Denial) -> Failure {
    let response = denial_response(&denial);
    match HttpView::of(cx) {
        Ok(view) => Failure::from(denial).answered(view.respond(response)),
        Err(viewless) => Failure::from(viewless),
    }
}

/// `inner` behind `chain`, run as the guard stage of a site `handler` declares
/// and nothing else: the seam a guard stage's own tests drive a request
/// through, as a self-mount's request runs it.
pub fn behind_guards(
    handler: &'static Handler,
    chain: Vec<ResolvedLayer<dyn Guard>>,
    inner: crate::endpoint::BoxEndpoint,
) -> crate::endpoint::BoxEndpoint {
    use nest_rs_core::__private::{GlobalLayers, SiteDeclaration, SiteLayers, compose_site};

    let stage: Arc<dyn GuardStage> =
        Arc::new(HttpGuardStage::new(chain, Vec::new(), StageKind::SelfMount));
    let declared = SiteDeclaration {
        layers: SiteLayers::default,
        global: GlobalLayers::NONE,
        guard_stage: None,
        class_gate: None,
    };
    // Composing around a given stage resolves nothing: it cannot refuse.
    #[expect(
        clippy::expect_used,
        reason = "a site with no layer of its own to resolve composes"
    )]
    let chains = compose_site(
        &nest_rs_core::Container::default(),
        handler,
        &declared,
        Some(stage),
    )
    .expect("a site with nothing to resolve composes");
    crate::endpoint::BoxEndpoint::new(crate::site::SiteEndpoint::new(
        handler,
        chains,
        None,
        crate::site::EndpointTail(inner),
    ))
}

/// Run `guards`' HTTP entry on `req`, the unit `handler` declares, as the
/// transport's stage runs them, keeping what they attach on the request
/// ([`attached`]): the seam a guard's tests check a request through.
///
/// A refusal is filed here, as on a route, and comes back as raised.
pub async fn check_request<'g>(
    handler: &'static Handler,
    guards: impl IntoIterator<Item = (&'static str, &'g dyn Guard)>,
    req: &mut Request,
) -> Result<(), Denial> {
    let earlier = pending_in(req.extensions());
    let taken = std::mem::replace(req, Request::builder().finish());
    let mut view = HttpView::new(taken);
    let checked = install_pending(earlier, async {
        let mut cx = new_unit_context(handler, &mut view, None, None);
        for (name, guard) in guards {
            guard
                .check_http(&mut cx)
                .await
                .map_err(|denial| Refused::by(name, guard, denial))?;
        }
        Ok(take_pending(&mut cx))
    })
    .await;
    *req = view.into_request();
    let pending = checked.map_err(|refused: Refused| refused.into_denial(file_denial))?;
    let attached = req.extensions_mut();
    match attached.get_mut::<Attached>() {
        Some(Attached(earlier)) => earlier.merge(pending),
        None => {
            attached.insert(Attached(pending));
        }
    }
    Ok(())
}

/// What `req`'s guard stages attached so far, to install around what runs
/// after them — later stages, work that leaves the request's task (an MCP
/// operation).
pub fn attached_pending(req: &Request) -> Pending {
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
pub fn attached<T: AmbientValue>(req: &Request) -> Option<Arc<T>> {
    req.extensions().get::<Attached>()?.0.get::<T>()
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
    denial_response(&denial)
}

/// [`denial_to_http_response`], the denial kept.
pub(crate) fn denial_response(denial: &Denial) -> Response {
    let mut response = problem_response(denial);
    // The scopes ride as an extension: the challenge also names the RFC 9728
    // metadata document, which only the oauth-resource interceptor knows.
    if let Some(required) = required_scopes(denial) {
        response.extensions_mut().insert(required);
    }
    response
}

/// The problem+json envelope alone — everything [`denial_to_http_response`]
/// builds except the scope evidence, which the `Err` path attaches to the
/// [`HttpError`] instead of to this response.
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
    match denial.bearer_error() {
        Some(crate::challenge::INSUFFICIENT_SCOPE) => {
            crate::challenge::stamp_insufficient_scope(&mut response, denial.required_scopes());
        }
        Some(code) => crate::challenge::stamp_bearer_error(&mut response, code),
        None => {}
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

/// A [`Denial`] as an [`HttpError`] — the `Err` path's counterpart to
/// [`denial_to_http_response`], for the sites that must reject rather than
/// return (an extractor). The scope evidence rides the error's extensions,
/// which its rendering copies onto the response.
pub fn denial_to_http_error(denial: Denial) -> HttpError {
    let mut error = HttpError::from_response(problem_response(&denial));
    if let Some(required) = required_scopes(&denial) {
        error.extensions_mut().insert(required);
    }
    error
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::__private::{
        GlobalLayers, HandlerDeclaration, MetaLevels, SiteDeclaration, SiteLayers, compose_site,
        declare_handler, meta_value,
    };
    use nest_rs_core::{Container, Layer, LayerSite, Posture, UnitContext};
    use nest_rs_guards::async_trait;

    use super::*;
    use crate::endpoint::{BoxEndpoint, Endpoint};
    use crate::site::{EndpointTail, SiteEndpoint};

    /// The guards of a composed chain, as `check_request` takes them.
    fn erased(
        chain: &[ResolvedLayer<dyn Guard>],
    ) -> impl Iterator<Item = (&'static str, &dyn Guard)> {
        chain.iter().map(|entry| (entry.name, entry.layer.as_ref()))
    }

    /// `inner` behind a site running `chain` as its guard stage, a `kind`
    /// stage, and nothing else.
    fn guarded(
        chain: Vec<ResolvedLayer<dyn Guard>>,
        kind: StageKind,
        inner: impl Endpoint,
    ) -> impl Endpoint {
        let container = Container::default();
        let stage: Arc<dyn GuardStage> = Arc::new(HttpGuardStage::new(chain, Vec::new(), kind));
        let chains = compose_site(
            &container,
            &ROUTE,
            &SiteDeclaration {
                layers: SiteLayers::default,
                global: GlobalLayers::NONE,
                guard_stage: None,
                class_gate: None,
            },
            Some(stage),
        )
        .expect("the site composes");
        SiteEndpoint::new(&ROUTE, chains, None, EndpointTail(BoxEndpoint::new(inner)))
    }

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
        assert_eq!(
            resp.headers()[header::WWW_AUTHENTICATE],
            r#"Bearer error="insufficient_scope", scope="posts:write""#,
            "RFC 6750 §3: the challenge names the scope the request needs",
        );
    }

    #[tokio::test]
    async fn the_err_path_carries_the_scopes_to_the_response_it_renders() {
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
        tenant("acme")
    }

    fn tenant(name: &'static str) -> Request {
        Request::builder()
            .header(
                http::HeaderName::from_static("x-tenant"),
                HeaderValue::from_static(name),
            )
            .finish()
    }

    #[tokio::test]
    async fn a_later_stage_reads_what_an_earlier_one_attached_and_never_a_shadow() {
        let mut req = tenant_request();
        check_request(&ROUTE, erased(&chain(TenantGuard)), &mut req)
            .await
            .expect("the tenant is known");
        check_request(&ROUTE, erased(&chain(Witness)), &mut req)
            .await
            .expect("the second stage reads the first one's tenant and the declared throttle");

        let seen = install_pending(attached_pending(&req), async {
            nest_rs_core::ambient::<Tenant>()
        })
        .await;
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
        let mut req = tenant("globex");
        let denial = check_request(&ROUTE, erased(&chain(TenantGuard)), &mut req)
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
    async fn a_guarded_site_runs_its_tail_under_what_the_stage_attached() {
        let inner = crate::endpoint_fn(|_req: Request| async {
            let tenant = nest_rs_core::ambient::<Tenant>()
                .map(|tenant| tenant.0)
                .unwrap_or("none");
            Ok(Response::builder().body(tenant))
        });
        let guarded = guarded(chain(TenantGuard), StageKind::SelfMount, inner);

        let admitted = guarded.call(tenant_request()).await.expect("answers");
        assert_eq!(
            admitted.into_body().into_string().await.expect("a body"),
            "acme"
        );

        let logs = nest_rs_testing::LogCapture::install();
        let refused = guarded
            .call(Request::builder().finish())
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
        let inner =
            crate::endpoint_fn(|_req: Request| async { Ok(Response::builder().body("reached")) });
        let guarded = guarded(
            chain(StoreDown),
            StageKind::Route { unmasked: false },
            inner,
        );

        let refused = guarded
            .call(Request::builder().finish())
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
        let denial = check_request(
            &ROUTE,
            erased(&chain(StoreDown)),
            &mut Request::builder().finish(),
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
    async fn withheld(chain: Vec<ResolvedLayer<dyn Guard>>, req: Request) -> bool {
        let rest = crate::endpoint_fn(|_req| async {
            let marked = nest_rs_core::ambient::<UnmaskedReply>().is_some();
            Ok(Response::builder().body(if marked { "withheld" } else { "bare" }))
        });
        let answer = guarded(chain, StageKind::Route { unmasked: true }, rest)
            .call(req)
            .await
            .expect("answers");
        answer.into_body().into_string().await.expect("a body") == "withheld"
    }

    #[tokio::test]
    async fn an_unmasked_route_marks_only_a_unit_carrying_something_to_withhold() {
        assert!(
            !withheld(chain(Admitting), Request::builder().finish()).await,
            "its guard attached nothing",
        );
        assert!(
            withheld(chain(TenantGuard), tenant_request()).await,
            "what its guard attached is withheld",
        );
        let carried = nest_rs_core::with_ambient(
            Arc::new(Tenant("acme")),
            withheld(chain(Admitting), Request::builder().finish()),
        )
        .await;
        assert!(carried, "what the unit carried in is withheld too");
    }

    #[tokio::test]
    async fn a_self_mounts_stage_keeps_what_it_attached_on_the_request_for_its_off_task_work() {
        let inner = crate::endpoint_fn(|req: Request| async move {
            let kept = attached::<Tenant>(&req).map_or("none", |tenant| tenant.0);
            Ok(Response::builder().body(kept))
        });
        let answer = guarded(chain(TenantGuard), StageKind::SelfMount, inner)
            .call(tenant_request())
            .await
            .expect("answers");
        assert_eq!(
            answer.into_body().into_string().await.expect("a body"),
            "acme"
        );
    }
}
