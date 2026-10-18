//! One unit of the HTTP edge — a routed request, a self-mount's request, what
//! the router answers itself — run through the pipeline: its site's chains,
//! composed once at mount, dispatched inside the unit of work the request
//! runs, with the request's deadline handed to that unit, and its answer
//! rendered by the edge's one rule.

use std::future::Future;
use std::sync::Arc;

use nest_rs_core::__private::{
    Shape, Tail, UnitChains, caught_problem, dispatch, new_unit_context, withheld,
};
use nest_rs_core::{DecodeError, Failure, Handler, Outcome, Peer, Problem, Settled, UnitContext};
use nest_rs_database::{
    ExecutorScope, TransactionPolicy, Unhonoured, UnitOfWork, UnitSpec, run_unit,
};
use nest_rs_guards::Denial;
use nest_rs_pipes::PipeError;

use crate::client_ip::ClientOrigin;
use crate::deadline::RequestDeadline;
use crate::endpoint::Endpoint;
use crate::error::{HttpError, Result, failed, file};
use crate::guard_stage::denial_response;
use crate::pipe::rejection;
use crate::problem::{ProblemDetails, problem_response};
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::view::HttpView;

/// What a site runs last: the handler, a mount's endpoint, the router's own
/// answer.
pub(crate) trait SiteTail: Send + Sync + 'static {
    /// Answer `req`.
    fn answer(&self, req: Request) -> impl Future<Output = Result<Response>> + Send;
}

/// A mount's endpoint as a site's tail: a self-mount's entry, the router's
/// fallback.
pub(crate) struct EndpointTail(pub(crate) crate::endpoint::BoxEndpoint);

impl SiteTail for EndpointTail {
    fn answer(&self, req: Request) -> impl Future<Output = Result<Response>> + Send {
        self.0.call(req)
    }
}

/// How the unit of work a request runs is settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnitPolicy {
    /// One transaction for a method that changes state, the pool for a safe
    /// one, its body cut at the request's deadline.
    ByMethod,
    /// The pool whatever the method, its cut left to the edge: the units the
    /// request carries — GraphQL's, MCP's operations — settle their own work.
    Carried,
}

/// One HTTP site, mounted: what its unit runs through, and its tail.
pub(crate) struct SiteEndpoint<T> {
    handler: &'static Handler,
    chains: Arc<UnitChains>,
    /// The binding the unit opens through, with its policy; `None` when no
    /// binding is imported or the site runs no unit of work.
    unit: Option<(Arc<dyn UnitOfWork>, UnitPolicy)>,
    tail: T,
}

impl<T: SiteTail> SiteEndpoint<T> {
    /// The site `handler` declares, running `chains` around `tail`, inside a
    /// unit of `unit`'s binding when one is given.
    pub(crate) fn new(
        handler: &'static Handler,
        chains: Arc<UnitChains>,
        unit: Option<(Arc<dyn UnitOfWork>, UnitPolicy)>,
        tail: T,
    ) -> Self {
        Self {
            handler,
            chains,
            unit,
            tail,
        }
    }
}

/// What one unit answered, and whether its unit of work commits.
struct Answered {
    response: Response,
    commits: bool,
}

/// A unit commits on a success: a handler error, a refusal or a `4xx`/`5xx`
/// it answered — however a layer answered it — rolls it back.
fn commits(answered: &Answered) -> bool {
    answered.commits
}

/// What a unit whose binding could not honour it answers: the opaque `500`,
/// the driver having said why.
fn unhonoured(_: Unhonoured) -> Answered {
    Answered {
        response: ProblemDetails::internal().into_response(),
        commits: false,
    }
}

impl<T: SiteTail> Endpoint for SiteEndpoint<T> {
    async fn call(&self, req: Request) -> Result<Response> {
        match &self.unit {
            // Nothing bound and no unit of work: the tail alone, as it answers.
            None if self.chains.shape() == Shape::Bare => self.tail.answer(req).await,
            None => Ok(self.dispatched(req).await.response),
            Some((unit, policy)) => Ok(self.in_unit(unit, *policy, req).await),
        }
    }
}

impl<T: SiteTail> SiteEndpoint<T> {
    /// The unit inside a unit of work of `unit`'s binding, cut at the
    /// request's deadline when `policy` cuts it there.
    async fn in_unit(
        &self,
        unit: &Arc<dyn UnitOfWork>,
        policy: UnitPolicy,
        req: Request,
    ) -> Response {
        let (transaction, cuts) = match policy {
            UnitPolicy::ByMethod if req.method().is_safe() => (TransactionPolicy::Pool, true),
            UnitPolicy::ByMethod => (TransactionPolicy::PerUnit, true),
            UnitPolicy::Carried => (TransactionPolicy::Pool, false),
        };
        let spec = UnitSpec::new(crate::unit::REQUEST, ExecutorScope::Request, transaction);
        let deadline = req
            .extensions()
            .get::<RequestDeadline>()
            .filter(|_| cuts)
            .cloned();
        let Some(deadline) = deadline else {
            return run_unit(Some(unit), spec, self.dispatched(req), commits, unhonoured)
                .await
                .response;
        };
        // The unit cuts its body at the deadline, and the edge then waits its
        // settle: a commit is never cut.
        deadline.hand_to_unit(unit.settle_net());
        let at = deadline.at();
        let cut = async move {
            match tokio::time::timeout_at(at, self.dispatched(req)).await {
                Ok(answered) => answered,
                Err(_) => Answered {
                    response: deadline.timed_out(),
                    commits: false,
                },
            }
        };
        run_unit(Some(unit), spec.with_deadline(at), cut, commits, unhonoured)
            .await
            .response
    }

    /// The unit through its chains, answered.
    async fn dispatched(&self, req: Request) -> Answered {
        if self.chains.shape() == Shape::Bare {
            return match self.tail.answer(req).await {
                Ok(response) => Answered {
                    commits: !is_error(&response),
                    response,
                },
                Err(error) => Answered {
                    response: error.answer(),
                    commits: false,
                },
            };
        }
        let peer = req
            .extensions()
            .get::<ClientOrigin>()
            .map(|origin| Peer::new(origin.client(), None));
        let deadline = req
            .extensions()
            .get::<RequestDeadline>()
            .map(RequestDeadline::at);
        let mut view = HttpView::new(req);
        let outcome = {
            let mut cx = new_unit_context(self.handler, &mut view, peer, deadline);
            dispatch(&mut cx, &self.chains, &mut Running(&self.tail)).await
        };
        let commits = outcome.is_ok();
        Answered {
            response: render(&mut view, outcome),
            commits,
        }
    }
}

/// Whether `response` answers an error: a `4xx` or a `5xx`.
fn is_error(response: &Response) -> bool {
    let status = response.status();
    status.is_client_error() || status.is_server_error()
}

/// A site's tail as the pipeline runs it: the request taken out of the view,
/// the answer written back into it. A handler's error, or a `4xx`/`5xx` it
/// answered, fails the unit, answered: the exception filters see it, and a
/// failed unit never commits.
struct Running<'t, T>(&'t T);

impl<T: SiteTail> Tail for Running<'_, T> {
    fn call(&mut self, cx: &mut UnitContext<'_>) -> impl Future<Output = Outcome> + Send {
        let tail = self.0;
        async move {
            let req = HttpView::of(cx)?.take_request();
            let answered = tail.answer(req).await;
            let view = HttpView::of(cx)?;
            match answered {
                Ok(response) => {
                    let status = response.status();
                    let failed = is_error(&response);
                    let settled = view.respond(response);
                    match failed {
                        false => Ok(settled),
                        true => {
                            Err(Failure::from(HttpError::from_status(status)).answered(settled))
                        }
                    }
                }
                Err(error) => {
                    let (response, error) = error.answer_kept();
                    let settled = view.respond(response);
                    Err(Failure::from(error).answered(settled))
                }
            }
        }
    }

    fn answer_failure(&mut self, cx: &mut UnitContext<'_>, failure: &Failure) -> Option<Settled> {
        let response = failure_response(failure);
        HttpView::of(cx).ok().map(|view| view.respond(response))
    }
}

/// The response `outcome` answers: the reply the view holds, an exception
/// filter's problem over it, or, for a failure nobody answered, the edge's one
/// rule.
fn render(view: &mut HttpView, outcome: Outcome) -> Response {
    let failure = match outcome {
        Ok(_) => {
            return view.take_response().unwrap_or_else(|| {
                tracing::error!(
                    target: crate::target::HTTP,
                    "a unit settled without a response; answered 500",
                );
                ProblemDetails::internal().into_response()
            });
        }
        Err(failure) => failure,
    };
    if failure.is_answered()
        && let Some(response) = view.take_response()
    {
        return response;
    }
    failure_response(&failure)
}

/// What a failure the view holds no answer to renders: an exception filter's
/// problem, or the edge's one rule.
fn failure_response(failure: &Failure) -> Response {
    match caught_problem(failure) {
        Some(problem) => {
            if let Some(withheld) = withheld(problem, failure.error()) {
                file(withheld);
            }
            problem_response(problem)
        }
        None => unanswered(failure),
    }
}

/// What a failure no layer answered renders: a refusal, a client error the
/// framework names or a problem in its structured form, anything else the
/// opaque `500` with its chain filed once at `error`.
fn unanswered(failure: &Failure) -> Response {
    if let Some(denial) = failure.find::<Denial>() {
        return denial_response(denial);
    }
    if let Some(problem) = failure.find::<Problem>() {
        if let Some(withheld) = withheld(problem, failure.error()) {
            file(withheld);
        }
        return problem_response(problem);
    }
    if let Some(refused) = failure.find::<PipeError>() {
        return rejection(refused.clone()).into_response();
    }
    if let Some(decode) = failure.find::<DecodeError>() {
        return ProblemDetails::bad_request()
            .with_detail(decode.to_string())
            .into_response();
    }
    if let Some(error) = failure.find::<HttpError>() {
        return error.answer_ref();
    }
    failed(&nest_rs_core::error_message(failure.error()));
    ProblemDetails::internal().into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use http::{Method, StatusCode, header};
    use nest_rs_core::__private::{
        GlobalLayers, HandlerDeclaration, MetaLevels, SiteDeclaration, SiteLayers, compose_site,
        declare_handler,
    };
    use nest_rs_core::{Container, Posture};
    use nest_rs_database::{Executor, OpenUnit, Settlement};
    use tokio::time::Instant;

    use super::*;
    use crate::endpoint::{BoxEndpoint, endpoint_fn};

    struct Routes;

    static ROUTE: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::REQUEST,
        host: std::any::TypeId::of::<Routes>(),
        host_name: "Routes",
        method: "route",
        address: "/x",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    /// A binding recording each unit it opens and how each settled, settling
    /// on a task of its own after `takes`, answering `answer`.
    struct Recording {
        takes: Duration,
        answer: Settlement,
        units: Mutex<Vec<(TransactionPolicy, Option<Instant>)>>,
        settled: Arc<Mutex<Vec<bool>>>,
    }

    impl Recording {
        fn new(takes: Duration, answer: Settlement) -> Arc<Self> {
            Arc::new(Self {
                takes,
                answer,
                units: Mutex::default(),
                settled: Arc::default(),
            })
        }

        fn units(&self) -> Vec<(TransactionPolicy, Option<Instant>)> {
            self.units
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn settled(&self) -> Vec<bool> {
            self.settled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    struct Nowhere;

    impl Executor for Nowhere {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct Opened {
        takes: Duration,
        answer: Settlement,
        settled: Arc<Mutex<Vec<bool>>>,
    }

    impl UnitOfWork for Recording {
        fn open(&self, spec: &UnitSpec) -> std::result::Result<Box<dyn OpenUnit>, Unhonoured> {
            self.units
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((spec.policy, spec.deadline));
            Ok(Box::new(Opened {
                takes: self.takes,
                answer: self.answer,
                settled: Arc::clone(&self.settled),
            }))
        }

        fn settle_net(&self) -> Duration {
            Duration::from_secs(16)
        }
    }

    impl OpenUnit for Opened {
        fn executor(&self) -> Arc<dyn Executor> {
            Arc::new(Nowhere)
        }

        fn settle(
            self: Box<Self>,
            ok: bool,
        ) -> std::pin::Pin<Box<dyn Future<Output = Settlement> + Send>> {
            let Self {
                takes,
                answer,
                settled,
            } = *self;
            settled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(ok);
            let settling = tokio::spawn(async move {
                tokio::time::sleep(takes).await;
                answer
            });
            Box::pin(async move { settling.await.unwrap_or(Settlement::Settled) })
        }
    }

    /// A site running nothing but its tail, inside `unit`'s units by `policy`.
    fn site(
        unit: &Arc<Recording>,
        policy: UnitPolicy,
        tail: impl Endpoint,
    ) -> SiteEndpoint<EndpointTail> {
        let chains = compose_site(
            &Container::default(),
            &ROUTE,
            &SiteDeclaration {
                layers: SiteLayers::default,
                global: GlobalLayers::NONE,
                guard_stage: None,
                class_gate: None,
            },
            None,
        )
        .expect("the site composes");
        let unit: Arc<dyn UnitOfWork> = Arc::clone(unit) as Arc<dyn UnitOfWork>;
        SiteEndpoint::new(
            &ROUTE,
            chains,
            Some((unit, policy)),
            EndpointTail(BoxEndpoint::new(tail)),
        )
    }

    /// A `method` request whose edge set its deadline one second from now.
    fn under_a_deadline(method: Method) -> (Request, RequestDeadline) {
        let deadline = RequestDeadline::new(
            Instant::now() + Duration::from_secs(1),
            Duration::from_secs(1),
        );
        let req = Request::builder()
            .method(method)
            .extension(deadline.clone())
            .finish();
        (req, deadline)
    }

    fn answering_after(after: Duration, status: StatusCode) -> impl Endpoint {
        endpoint_fn(move |_req| async move {
            tokio::time::sleep(after).await;
            Ok(Response::builder().status(status).finish())
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_past_the_deadline_answers_the_handlers_status() {
        let started = Instant::now();
        let unit = Recording::new(Duration::from_secs(2), Settlement::Settled);
        let (req, deadline) = under_a_deadline(Method::POST);
        let answered = site(
            &unit,
            UnitPolicy::ByMethod,
            answering_after(Duration::from_millis(500), StatusCode::CREATED),
        )
        .call(req)
        .await
        .expect("answers");
        assert_eq!(
            answered.status(),
            StatusCode::CREATED,
            "never a 503 for a write that landed"
        );
        assert_eq!(started.elapsed(), Duration::from_millis(2500));
        assert_eq!(
            deadline.unit_net(),
            Some(Duration::from_secs(16)),
            "the edge waits the unit's net past the deadline"
        );
        assert_eq!(
            unit.units(),
            [(TransactionPolicy::PerUnit, Some(deadline.at()))]
        );
        assert_eq!(unit.settled(), [true]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_body_past_the_deadline_is_cut_and_answered_503_with_a_retry_after() {
        let started = Instant::now();
        let unit = Recording::new(Duration::from_secs(5), Settlement::Settled);
        let (req, _) = under_a_deadline(Method::POST);
        let answered = site(
            &unit,
            UnitPolicy::ByMethod,
            answering_after(Duration::from_secs(30), StatusCode::CREATED),
        )
        .call(req)
        .await
        .expect("a cut body is an answer");
        assert_eq!(answered.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            answered
                .headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"1".as_slice()),
            "honest now: nothing it ran committed"
        );
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(1),
            "answered at the deadline, its rollback left to the driver"
        );
        assert_eq!(unit.settled(), [false]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_carrying_units_runs_on_the_pool_and_leaves_its_cut_to_them() {
        let started = Instant::now();
        let unit = Recording::new(Duration::ZERO, Settlement::Settled);
        let (req, deadline) = under_a_deadline(Method::POST);
        let answered = site(
            &unit,
            UnitPolicy::Carried,
            answering_after(Duration::from_secs(3), StatusCode::CREATED),
        )
        .call(req)
        .await
        .expect("answers");
        assert_eq!(answered.status(), StatusCode::CREATED);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(3),
            "not cut here: the units it carries cut their own bodies"
        );
        assert_eq!(deadline.unit_net(), None, "nor taken over");
        assert_eq!(
            unit.units(),
            [(TransactionPolicy::Pool, None)],
            "a POST, yet its own unit is the pool"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_in_doubt_answers_500_without_a_retry_after() {
        let unit = Recording::new(
            Duration::from_millis(100),
            Settlement::Unhonoured(Unhonoured::IN_DOUBT),
        );
        let (req, _) = under_a_deadline(Method::POST);
        let answered = site(
            &unit,
            UnitPolicy::ByMethod,
            answering_after(Duration::from_millis(100), StatusCode::CREATED),
        )
        .call(req)
        .await
        .expect("answers");
        assert_eq!(answered.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(answered.headers().get(header::RETRY_AFTER).is_none());
    }

    #[tokio::test]
    async fn a_safe_method_runs_on_the_pool_and_an_error_status_rolls_back() {
        let unit = Recording::new(Duration::ZERO, Settlement::Settled);
        for (method, status, policy, commits) in [
            (Method::GET, StatusCode::OK, TransactionPolicy::Pool, true),
            (
                Method::POST,
                StatusCode::SEE_OTHER,
                TransactionPolicy::PerUnit,
                true,
            ),
            (
                Method::POST,
                StatusCode::NOT_FOUND,
                TransactionPolicy::PerUnit,
                false,
            ),
            (
                Method::DELETE,
                StatusCode::INTERNAL_SERVER_ERROR,
                TransactionPolicy::PerUnit,
                false,
            ),
        ] {
            let answered = site(
                &unit,
                UnitPolicy::ByMethod,
                answering_after(Duration::ZERO, status),
            )
            .call(Request::builder().method(method.clone()).finish())
            .await
            .expect("answers");
            assert_eq!(answered.status(), status, "{method}");
            assert_eq!(
                unit.units().last().map(|unit| unit.0),
                Some(policy),
                "{method}"
            );
            assert_eq!(unit.settled().last(), Some(&commits), "{method} {status}");
        }
    }

    #[tokio::test]
    async fn a_handler_error_rolls_back_and_answers_its_own_rendering() {
        let unit = Recording::new(Duration::ZERO, Settlement::Settled);
        let failing = endpoint_fn(|_req| async {
            Err(HttpError::from(Problem::new(
                409,
                nest_rs_core::problem::code::CONFLICT,
            )))
        });
        let answered = site(&unit, UnitPolicy::ByMethod, failing)
            .call(Request::builder().method(Method::POST).finish())
            .await
            .expect("answers");
        assert_eq!(answered.status(), StatusCode::CONFLICT);
        assert_eq!(unit.settled(), [false]);
    }
}
