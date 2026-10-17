//! Safe or mutating is read off the HTTP method; a request whose self-mount
//! carries units of its own (`/graphql`, `/mcp`) runs on the pool.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_http::CarriedUnits;
use nest_rs_http::RequestDeadline;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::http::{Method, StatusCode};
use poem::{Error, Request, Response, Result};
use sea_orm::DatabaseConnection;

use crate::{SeaOrmConfig, SeaOrmUnitOfWork};

/// The request interceptor that runs each HTTP request as one unit of work
/// through the bound [`UnitOfWork`]: the pool for a safe method, a **lazily
/// opened** transaction (committed on 2xx/3xx, rolled back otherwise; never
/// opened when nothing touches the data layer) for a mutating one.
/// Auto-mounted at band −10 by importing
/// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule).
///
/// It takes the request timeout over from the edge: the handler alone is cut
/// at the deadline, answered `503` with `Retry-After` since nothing it ran
/// committed, and a commit is never cut — one past the deadline answers the
/// handler's status, and one that never answers is a `500`.
///
/// A request to a surface whose operations are units of their own — a GraphQL
/// operation, an MCP operation — runs on the pool whatever its method: its
/// guards read there, each operation settles its own work, and the cut is left
/// to the edge or to the operations.
#[interceptor(priority = -10)]
pub struct DbContext {
    #[inject]
    unit: Arc<dyn UnitOfWork>,
}

impl DbContext {
    /// The interceptor over SeaORM's unit of work on `db`, configured by
    /// `config`.
    pub fn new(db: Arc<DatabaseConnection>, config: Arc<SeaOrmConfig>) -> Self {
        Self {
            unit: Arc::new(SeaOrmUnitOfWork::new(db, &config)),
        }
    }
}

impl Layer for DbContext {}

#[async_trait]
impl Interceptor for DbContext {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        let carries_units = req.extensions().get::<CarriedUnits>().is_some();
        let policy = if carries_units || is_safe(req.method()) {
            TransactionPolicy::Pool
        } else {
            TransactionPolicy::PerUnit
        };
        let spec = UnitSpec::new(nest_rs_http::unit::REQUEST, ExecutorScope::Request, policy);
        let deadline = req
            .extensions()
            .get::<RequestDeadline>()
            .filter(|_| !carries_units)
            .cloned();
        let Some(deadline) = deadline else {
            return run_unit(
                Some(&self.unit),
                spec,
                next.run(req),
                should_commit,
                unhonoured,
            )
            .await;
        };
        deadline.hand_to_unit(self.unit.settle_net());
        let at = deadline.at();
        let cut = async move {
            tokio::time::timeout_at(at, next.run(req))
                .await
                .unwrap_or_else(|_| Ok(deadline.timed_out().into()))
        };
        run_unit(
            Some(&self.unit),
            spec.with_deadline(at),
            cut,
            should_commit,
            unhonoured,
        )
        .await
    }
}

/// A success the unit could not honour reports writes that were never
/// committed, or may have been: the driver logged why.
fn unhonoured(_: nest_rs_database::Unhonoured) -> Result<Response> {
    Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
}

fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// 2xx and 3xx commit, unless tagged [`MappedError`](nest_rs_http::MappedError):
/// a filter mapping a handler error does not bless its writes.
fn should_commit(result: &Result<Response>) -> bool {
    matches!(
        result,
        Ok(resp) if (resp.status().is_success() || resp.status().is_redirection())
            && resp.extensions().get::<nest_rs_http::MappedError>().is_none()
    )
}

#[cfg(test)]
mod tests {
    use poem::IntoResponse;

    use super::*;

    #[test]
    fn safe_methods_skip_the_transaction_wrapper() {
        assert!(is_safe(&Method::GET));
        assert!(is_safe(&Method::HEAD));
        assert!(is_safe(&Method::OPTIONS));
        assert!(is_safe(&Method::TRACE));
    }

    #[test]
    fn mutating_methods_open_a_transaction() {
        assert!(!is_safe(&Method::POST));
        assert!(!is_safe(&Method::PUT));
        assert!(!is_safe(&Method::PATCH));
        assert!(!is_safe(&Method::DELETE));
    }

    fn response_with(status: StatusCode) -> Result<Response> {
        Ok(status.into_response())
    }

    #[test]
    fn two_xx_commits() {
        assert!(should_commit(&response_with(StatusCode::OK)));
        assert!(should_commit(&response_with(StatusCode::CREATED)));
        assert!(should_commit(&response_with(StatusCode::NO_CONTENT)));
    }

    #[test]
    fn three_xx_commits() {
        assert!(should_commit(&response_with(StatusCode::MOVED_PERMANENTLY)));
        assert!(should_commit(&response_with(StatusCode::SEE_OTHER)));
    }

    #[test]
    fn four_xx_and_five_xx_roll_back() {
        assert!(!should_commit(&response_with(StatusCode::BAD_REQUEST)));
        assert!(!should_commit(&response_with(StatusCode::FORBIDDEN)));
        assert!(!should_commit(&response_with(
            StatusCode::INTERNAL_SERVER_ERROR,
        )));
    }

    #[test]
    fn err_rolls_back() {
        let err: Result<Response> = Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!should_commit(&err));
    }

    #[test]
    fn a_mapped_error_rolls_back_even_with_a_success_status() {
        let mut resp = StatusCode::OK.into_response();
        resp.extensions_mut().insert(nest_rs_http::MappedError);
        assert!(!should_commit(&Ok(resp)));
    }

    use std::time::Duration;

    use nest_rs_database::{Executor, OpenUnit, Settlement, Unhonoured};
    use nest_rs_http::RequestDeadline;
    use nest_rs_interceptors::InterceptorExt;
    use poem::Endpoint;
    use poem::endpoint::make;
    use tokio::time::Instant;

    /// A unit of work settling on a task of its own after `takes`, answering
    /// `answer`.
    struct Settling {
        takes: Duration,
        answer: Settlement,
    }

    struct Nowhere;

    impl Executor for Nowhere {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    impl UnitOfWork for Settling {
        fn open(&self, _: &UnitSpec) -> std::result::Result<Box<dyn OpenUnit>, Unhonoured> {
            Ok(Box::new(Self {
                takes: self.takes,
                answer: self.answer,
            }))
        }

        fn settle_net(&self) -> Duration {
            Duration::from_secs(16)
        }
    }

    impl OpenUnit for Settling {
        fn executor(&self) -> Arc<dyn Executor> {
            Arc::new(Nowhere)
        }

        fn settle(
            self: Box<Self>,
            _ok: bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Settlement> + Send>> {
            let Self { takes, answer } = *self;
            let settling = tokio::spawn(async move {
                tokio::time::sleep(takes).await;
                answer
            });
            Box::pin(async move { settling.await.unwrap_or(Settlement::Settled) })
        }
    }

    impl Settling {
        fn interceptor(takes: Duration, answer: Settlement) -> DbContext {
            DbContext {
                unit: Arc::new(Self { takes, answer }),
            }
        }
    }

    /// A `POST` whose edge set its deadline one second from now.
    fn posted_under_a_deadline() -> (Request, RequestDeadline) {
        let deadline = RequestDeadline::new(
            Instant::now() + Duration::from_secs(1),
            Duration::from_secs(1),
        );
        let mut req = Request::builder().method(Method::POST).finish();
        req.extensions_mut().insert(deadline.clone());
        (req, deadline)
    }

    fn answering_after(after: Duration) -> impl Endpoint<Output = Response> {
        make(move |_req: Request| async move {
            tokio::time::sleep(after).await;
            StatusCode::CREATED.into_response()
        })
    }

    fn status_of(result: Result<Response>) -> StatusCode {
        match result {
            Ok(resp) => resp.status(),
            Err(err) => err.into_response().status(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_past_the_deadline_answers_the_handlers_status() {
        let started = Instant::now();
        let (req, deadline) = posted_under_a_deadline();
        let ctx = Settling::interceptor(Duration::from_secs(2), Settlement::Settled);
        let answered = answering_after(Duration::from_millis(500))
            .interceptor(ctx)
            .call(req)
            .await;
        assert_eq!(
            status_of(answered),
            StatusCode::CREATED,
            "never a 503 for a write that landed"
        );
        assert_eq!(started.elapsed(), Duration::from_millis(2500));
        assert_eq!(
            deadline.unit_net(),
            Some(Duration::from_secs(16)),
            "the edge waits the unit's net past the deadline"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_body_past_the_deadline_is_cut_and_answered_503_with_a_retry_after() {
        let started = Instant::now();
        let (req, _) = posted_under_a_deadline();
        let ctx = Settling::interceptor(Duration::from_secs(5), Settlement::Settled);
        let answered = answering_after(Duration::from_secs(30))
            .interceptor(ctx)
            .call(req)
            .await
            .expect("a cut body is an answer");
        assert_eq!(answered.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            answered
                .headers()
                .get(poem::http::header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(b"1".as_slice()),
            "honest now: nothing it ran committed"
        );
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(1),
            "answered at the deadline, its rollback left to the driver"
        );
    }

    /// A binding recording the policy of every unit it opens.
    #[derive(Default)]
    struct Recording(std::sync::Mutex<Vec<TransactionPolicy>>);

    impl UnitOfWork for Recording {
        fn open(&self, spec: &UnitSpec) -> std::result::Result<Box<dyn OpenUnit>, Unhonoured> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec.policy);
            Ok(Box::new(Settling {
                takes: Duration::ZERO,
                answer: Settlement::Settled,
            }))
        }

        fn settle_net(&self) -> Duration {
            Duration::from_secs(16)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_carrying_units_runs_on_the_pool_and_leaves_its_cut_to_them() {
        let started = Instant::now();
        let (mut req, deadline) = posted_under_a_deadline();
        req.extensions_mut()
            .insert(nest_rs_http::__private::carried_units());
        let recording = Arc::new(Recording::default());
        let ctx = DbContext {
            unit: Arc::clone(&recording) as Arc<dyn UnitOfWork>,
        };
        let answered = answering_after(Duration::from_secs(3))
            .interceptor(ctx)
            .call(req)
            .await;
        assert_eq!(status_of(answered), StatusCode::CREATED);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(3),
            "not cut here: the units it carries cut their own bodies"
        );
        assert_eq!(deadline.unit_net(), None, "nor taken over");
        assert_eq!(
            *recording.0.lock().unwrap(),
            [TransactionPolicy::Pool],
            "a POST, yet its own unit is the pool"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_in_doubt_answers_500_without_a_retry_after() {
        let (req, _) = posted_under_a_deadline();
        let ctx = Settling::interceptor(
            Duration::from_millis(100),
            Settlement::Unhonoured(Unhonoured::IN_DOUBT),
        );
        let answered = answering_after(Duration::from_millis(100))
            .interceptor(ctx)
            .call(req)
            .await;
        let resp = match answered {
            Ok(resp) => resp,
            Err(err) => err.into_response(),
        };
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            resp.headers()
                .get(poem::http::header::RETRY_AFTER)
                .is_none()
        );
    }
}
