//! The graphql-ws half of the `/graphql` mount, and the per-item posture seam
//! `#[subscription]` expands into.
//!
//! A subscription is an operation, so it goes through the gate every other
//! operation goes through — the same [`GraphqlOperationGuard`], the same
//! `#[authorize]`/`#[public]`. What differs is *when* it answers: once at
//! subscribe, then repeatedly, for as long as the socket lives. That produces
//! the two obligations this module carries and the POST path does not:
//!
//! - **the guard's decision is made once and must keep applying**, so every
//!   item is filtered against the ability captured at subscribe
//!   ([`keep_masked_item`], fed by `nest_rs_authz::graphql::masked_item_for`);
//! - **the socket must not outlive that decision**, so it carries the same
//!   lifetime ceiling a WebSocket gateway does
//!   ([`GraphqlConfig::max_connection`](crate::GraphqlConfig::max_connection)).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_graphql::Executor;
use async_graphql::http::ALL_WEBSOCKET_PROTOCOLS;
use async_graphql_poem::{GraphQLProtocol, GraphQLWebSocket};
use poem::web::websocket::WebSocket;
use poem::{Endpoint, FromRequest, IntoResponse, Request, Response, Result};

use nest_rs_core::{Container, Correlation};
use tracing::Instrument;

use crate::config::GraphqlConfig;
use crate::context::OperationBridge;

/// The composed schema as a bare [`Executor`] — the seam a *subscriber* needs.
///
/// Queries and mutations are testable through the mounted endpoint, because
/// `TestApp`'s client speaks the protocol they use. A subscription's protocol is
/// graphql-ws, and the thing that speaks it
/// ([`async_graphql::http::WebSocket`], and `GraphQLWebSocket` above it) takes
/// an executor rather than a URL — so a witness that boots the documented wiring
/// and then asserts *what a subscriber actually receives* has to reach the
/// executor the mount serves.
///
/// It hands back the schema opaquely: the discovered roots stay `pub(crate)`,
/// and the only thing a caller can do with the value is execute against it,
/// which is the whole point.
#[doc(hidden)]
pub fn compose_schema(container: Container, config: &GraphqlConfig) -> impl Executor + 'static {
    crate::resolver::build_schema(container, config)
}

/// Keep or drop one masked subscription item.
///
/// Called per item by the `#[subscription]` expansion, so the three outcomes are
/// worded once rather than inlined into every operation:
///
/// - masked and allowed ⇒ push it;
/// - the subscriber's ability refuses the row ⇒ drop it. This is the steady
///   state of a stream two principals read differently, not an anomaly, so it
///   logs at `debug`;
/// - masking failed ⇒ drop it and say so at `warn`. A stream item has no error
///   channel of its own (the field's type is the item's, not a `Result`), so
///   failing closed *is* dropping it — and an item silently vanishing because a
///   wire value would not reconcile is exactly the thing an operator has to be
///   able to find.
#[doc(hidden)]
pub fn keep_masked_item<T>(
    operation: &'static str,
    masked: Result<Option<T>, async_graphql::Error>,
) -> Option<T> {
    match masked {
        Ok(Some(item)) => Some(item),
        Ok(None) => {
            tracing::debug!(
                target: crate::TARGET,
                operation,
                reason = "not_granted",
                "subscription item withheld",
            );
            None
        }
        Err(err) => {
            tracing::warn!(
                target: crate::TARGET,
                operation,
                reason = "mask_failed",
                error = %err.message,
                "subscription item withheld",
            );
            None
        }
    }
}

/// The data handle a socket runs on, taken from the **upgrade request** — the
/// last moment ambient state exists.
///
/// poem answers the `101` before the connection task runs, so the request
/// boundary's executor is already gone by the time an operation on that socket
/// resolves. Without this a `Repo`-backed subscription fails on every item for
/// want of one, which is fail-closed but useless.
///
/// Always **non-transactional**, and that is the load-bearing half. A request
/// transaction belongs to the request that opened it and is finalized when the
/// `101` returns; carrying it onto a connection that may live for hours would
/// pin a pooled connection for that whole time and write through a transaction
/// nobody will ever commit. [`non_transactional`] steps out of it; a handle that
/// is already the pool has nothing to step out of and is kept. The atomicity
/// given up here is atomicity nothing on this path can use — a subscription
/// reads, and a mutation is not a subscription.
///
/// [`non_transactional`]: nest_rs_database::Executor::non_transactional
fn socket_executor() -> Option<Arc<dyn nest_rs_database::Executor>> {
    nest_rs_database::current_executor()
        .map(|executor| executor.non_transactional().unwrap_or(executor))
}

/// The identity a socket runs under, taken from the **upgrade request** — the
/// same last moment [`socket_executor`] reads, and for the same reason.
///
/// The upgrade *is* an HTTP request, so the socket inherits that request's id,
/// and its actor: the guard that just ran resolved one, and nothing on this
/// socket ever re-authenticates anybody. Without it every subscription item,
/// every withheld row and every mask failure below files under no request at
/// all — poem answers the `101` before the connection task runs, so the request
/// boundary is gone by the time the first item is produced.
///
/// A mint is the honest answer where there is nothing to inherit — an endpoint
/// mounted outside the HTTP edge. The socket's own events still group with each
/// other, which is strictly more than nothing.
///
/// **Identity, never the upgrade's resources.** The socket already steps out of
/// the request transaction ([`socket_executor`]) and holds no request scope, for
/// one reason: a connection that may live for hours would pin whatever the
/// upgrade resolved for that whole time.
fn socket_correlation() -> Correlation {
    Correlation::inherited()
}

/// Run `serve` under the socket-lifetime ceiling.
///
/// Same control and same reasoning as a WebSocket gateway's: a principal
/// captured once at the upgrade would otherwise keep its privileges after token
/// expiry, logout or revocation, for as long as the peer holds the socket open.
/// When the deadline wins, `serve` is dropped — which drops the sink, closing
/// the socket through the same path a client `Close` takes. The peer must
/// re-upgrade, which re-runs the guard and re-checks `exp`.
///
/// The deadline is **absolute**, armed once: it is a ceiling on the
/// connection's life, not an idle timeout, so traffic never pushes it out.
/// `None` ⇒ unbounded, and `serve` runs exactly as it would without this.
///
/// Split out from the upgrade closure because it is the security control this
/// module exists to carry, and a control worth stating is worth asserting: see
/// the tests below.
async fn bounded_by<F: Future<Output = ()>>(serve: F, ttl: Option<Duration>) {
    let Some(ttl) = ttl else {
        return serve.await;
    };
    tokio::select! {
        () = serve => {}
        () = tokio::time::sleep(ttl) => tracing::info!(
            target: crate::TARGET,
            max_connection_secs = ttl.as_secs(),
            "closing subscription socket: max lifetime reached",
        ),
    }
}

/// The graphql-ws endpoint served on the same path as the POST endpoint.
///
/// It is reached through [`crate::module`]'s GET dispatcher, which sends a
/// WebSocket upgrade here and everything else to the playground — one URL, the
/// shape every graphql-ws client expects.
pub(crate) struct SubscriptionEndpoint<E> {
    executor: E,
    bridge: Arc<OperationBridge>,
    max_connection: Option<Duration>,
}

impl<E> SubscriptionEndpoint<E> {
    pub(crate) fn new(
        executor: E,
        bridge: Arc<OperationBridge>,
        max_connection: Option<Duration>,
    ) -> Self {
        Self {
            executor,
            bridge,
            max_connection,
        }
    }
}

impl<E: Executor> Endpoint for SubscriptionEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let (mut req, mut body) = req.split();
        // The guard runs on the **upgrade**, before any operation exists: this
        // is where the principal is established, exactly as `before` does on the
        // POST path. A denial is answered as an ordinary HTTP response, so the
        // socket is never opened.
        //
        // `around` runs too, below — over the whole socket rather than over one
        // operation.
        if let Some(guard) = &self.bridge.op_guard
            && let Err(resp) = guard.before(&mut req).await
        {
            return Ok(resp);
        }
        let websocket = WebSocket::from_request(&req, &mut body).await?;
        let protocol = GraphQLProtocol::from_request(&req, &mut body).await?;
        let data = self.bridge.connection_data(&req);
        let executor = self.executor.clone();
        let max_connection = self.max_connection;
        let socket_executor = socket_executor();
        // The guard's ambient state is installed around the **whole socket**,
        // not per operation: one upgrade, one principal, for every operation and
        // every item the connection carries. That is the same model a WebSocket
        // gateway uses, and it is what makes `Guard::check_graphql` and the
        // ability-scoped data layer read the same ability here as on the POST
        // path — `around` never runs otherwise, since a socket has no response
        // future to wrap.
        let guard = self.bridge.op_guard.clone();
        // Read here, on the request task, for the same reason the executor is.
        let correlation = socket_correlation();

        Ok(websocket
            .protocols(ALL_WEBSOCKET_PROTOCOLS)
            .on_upgrade(move |stream| async move {
                // One span for the whole socket, and that granularity is the
                // honest one: `serve()` is async-graphql's own loop, so this
                // crate never sees an operation boundary to open a span at.
                // Where it *does* see one — a WebSocket gateway's messages — the
                // message is the unit and the connection is a field. Here the
                // connection is all there is.
                let span = nest_rs_core::operation_span!(
                    target: crate::TARGET,
                    kind: nest_rs_core::operation_log::kind::SERVER,
                    crate::unit::SUBSCRIPTION,
                    &correlation,
                );
                let serve = GraphQLWebSocket::new(stream, executor, protocol)
                    .with_data(data)
                    .serve();
                let bounded: crate::BoxFuture<'_, ()> = Box::pin(bounded_by(serve, max_connection));
                let guarded: crate::BoxFuture<'_, ()> = match &guard {
                    Some(guard) => guard.around(&req, bounded),
                    None => bounded,
                };
                // Executor outermost, ability inside — the same nesting the POST
                // path uses (`without_transaction` over the guarded operation),
                // so the two cannot come to disagree about which is in scope.
                let served: crate::BoxFuture<'_, ()> = match socket_executor {
                    Some(executor) => {
                        Box::pin(nest_rs_database::with_request_executor(executor, guarded))
                    }
                    None => guarded,
                };
                let line = SubscriptionLine::new(correlation.clone());
                nest_rs_core::with_request_scope(None, correlation, async move {
                    let _line = line;
                    served.await;
                })
                .instrument(span)
                .await
            })
            .into_response())
    }
}

/// Files the subscription's line when the connection ends.
///
/// **On drop, not after the await**, and that distinction is the whole reason
/// this type exists rather than a `tracing::info!` at the end of the block: a
/// socket that is aborted — the app shutting down, the client vanishing, the
/// connection ceiling elapsing — never completes `serve()`, so its future is
/// dropped rather than finished. A line filed only on the graceful path reported
/// the half that was never in doubt, and reported nothing at all in the
/// integration suite, which is where the gap was found. HTTP's access log has
/// always used `Drop` for the same reason.
///
/// The correlation is held rather than read from the ambient context: a `Drop`
/// runs while the future is being torn down, which is not reliably inside the
/// scope that future installed.
struct SubscriptionLine {
    correlation: nest_rs_core::Correlation,
    started: std::time::Instant,
}

impl SubscriptionLine {
    fn new(correlation: nest_rs_core::Correlation) -> Self {
        Self {
            correlation,
            started: std::time::Instant::now(),
        }
    }
}

impl Drop for SubscriptionLine {
    fn drop(&mut self) {
        nest_rs_core::RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            tracing::info!(
                name: crate::unit::SUBSCRIPTION,
                target: nest_rs_core::operation_log::TARGET,
                message = crate::unit::SUBSCRIPTION,
                // Always `ok`: async-graphql owns the message loop, so this crate
                // sees no operation boundary and has no failure signal to report
                // — the same fact the span's own comment records. A closed socket
                // is not an error, and an aborted one is the deployment's news,
                // not the subscription's.
                outcome = nest_rs_core::operation_log::OK,
                // How long it stayed open, which on a subscription is the number
                // an operator actually reads.
                duration_ms = nest_rs_core::operation_log::duration_ms(self.started),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// The line is filed from `Drop`, so two properties matter more than what it
    /// says: it fires **once**, and it cannot panic.
    ///
    /// A panic inside a `Drop` that runs during an unwind aborts the process, and
    /// this `Drop` reaches a `tokio` task-local. A socket is torn down on paths
    /// this crate does not choose — the runtime shutting down, the connection
    /// ceiling elapsing, a handler unwinding — so "does it work on the happy
    /// path" is not the question.
    #[test]
    fn the_subscription_line_fires_once_and_cannot_panic_off_a_runtime() {
        let logs = nest_rs_testing::LogCapture::install();

        // No `#[tokio::test]`: this is the teardown case, where there may be no
        // runtime left to reach a task-local through.
        drop(SubscriptionLine::new(nest_rs_core::Correlation::mint()));

        let served = logs.find(
            nest_rs_core::operation_log::TARGET,
            crate::unit::SUBSCRIPTION,
        );
        assert_eq!(
            served.len(),
            1,
            "exactly one line per connection: {served:?}"
        );
        assert_eq!(
            served[0].field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::OK),
        );
        assert!(served[0].field("duration_ms").is_some());
    }

    /// And once inside a runtime too, since that is where it normally runs — the
    /// two paths reach the task-local differently and only one of them is the
    /// happy one.
    #[tokio::test]
    async fn the_subscription_line_fires_once_inside_a_runtime() {
        let logs = nest_rs_testing::LogCapture::install();
        let correlation = nest_rs_core::Correlation::mint();

        nest_rs_core::with_request_scope(None, correlation.clone(), async {
            let _line = SubscriptionLine::new(correlation);
        })
        .await;

        assert_eq!(
            logs.find(
                nest_rs_core::operation_log::TARGET,
                crate::unit::SUBSCRIPTION
            )
            .len(),
            1,
        );
    }

    /// A request's lazy transaction handle: `non_transactional` steps out of it
    /// onto the pool, exactly as the ORM's does.
    struct Transaction;
    /// The pool handle it steps out onto — already outside a transaction, so it
    /// has nothing to step out of.
    struct Pool;

    impl nest_rs_database::Executor for Transaction {
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
            Some(Arc::new(Pool))
        }
    }

    impl nest_rs_database::Executor for Pool {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// The rule a socket cannot break: a connection that may live for hours
    /// never carries the upgrade's transaction. Carrying it would pin a pooled
    /// connection for that whole time and write through a transaction the `101`
    /// already finalized.
    #[tokio::test]
    async fn a_socket_never_carries_the_upgrades_transaction() {
        let captured = nest_rs_database::with_request_executor(Arc::new(Transaction), async {
            socket_executor()
        })
        .await;

        let captured = captured.expect("the upgrade runs inside the request boundary");
        assert!(
            captured.as_any().is::<Pool>(),
            "the socket runs on the pool, not on the request transaction",
        );
    }

    /// The identity half of the same capture. Every event a subscription
    /// produces — an item withheld, a mask that failed — is filed in the trace
    /// that opened the socket, so "what did this subscriber see?" is one query
    /// against the trace the client already has from its upgrade.
    #[tokio::test]
    async fn a_socket_runs_in_the_upgrades_trace() {
        let upgrade = Correlation::mint();
        let captured =
            nest_rs_core::with_request_scope(None, upgrade.clone(), async { socket_correlation() })
                .await;

        assert_eq!(captured.trace_id(), upgrade.trace_id());
    }

    /// And its actor, because nothing on a socket re-authenticates: what the
    /// upgrade's guard resolved is the only answer there will ever be.
    #[tokio::test]
    async fn a_socket_runs_under_the_upgrades_actor() {
        let captured = nest_rs_core::with_request_scope(None, Correlation::mint(), async {
            nest_rs_core::set_actor_id("alice-42");
            socket_correlation()
        })
        .await;

        assert_eq!(captured.actor_id(), Some("alice-42"));
    }

    /// Mounted outside the HTTP edge there is nothing to inherit. A fresh id
    /// still groups the socket's own events with each other, which is the point
    /// — an unidentified socket is the one outcome that helps nobody.
    #[tokio::test]
    async fn a_socket_with_no_upgrade_to_inherit_from_starts_its_own_trace() {
        assert!(nest_rs_core::current_trace_id().is_none());
        assert!(socket_correlation().actor_id().is_none());
    }

    /// The other half: a handle that is *already* the pool is kept, rather than
    /// dropped for want of something to step out of — which would leave a
    /// `Repo`-backed subscription with no executor at all.
    #[tokio::test]
    async fn a_socket_keeps_a_handle_that_is_already_the_pool() {
        let captured =
            nest_rs_database::with_request_executor(Arc::new(Pool), async { socket_executor() })
                .await;

        assert!(
            captured.is_some_and(|executor| executor.as_any().is::<Pool>()),
            "an upgrade on a safe method already holds the pool — keep it",
        );
    }

    /// No ORM installed at all: nothing to carry, and the socket runs untouched
    /// rather than failing to open.
    #[tokio::test]
    async fn a_socket_without_an_orm_carries_nothing() {
        assert!(socket_executor().is_none());
    }

    /// The ceiling ends a connection that would otherwise never end. Without
    /// this the control is only a config field: a socket whose peer keeps it
    /// open replays a principal captured hours ago, past token expiry, logout
    /// and revocation.
    #[tokio::test(start_paused = true)]
    async fn a_socket_that_never_ends_is_closed_when_the_ceiling_elapses() {
        let served = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&served);
        let forever = async move {
            std::future::pending::<()>().await;
            flag.store(true, Ordering::SeqCst);
        };

        bounded_by(forever, Some(Duration::from_secs(60))).await;

        assert!(
            !served.load(Ordering::SeqCst),
            "the deadline won, so the serve future was dropped rather than completed",
        );
    }

    /// A socket that closes on its own is not held open to the ceiling — the
    /// deadline is an upper bound, never a floor.
    #[tokio::test(start_paused = true)]
    async fn a_socket_that_ends_first_is_not_held_to_the_ceiling() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let closes = async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            flag.store(true, Ordering::SeqCst);
        };

        bounded_by(closes, Some(Duration::from_secs(60))).await;

        assert!(
            done.load(Ordering::SeqCst),
            "the serve future ran to completion"
        );
    }

    /// `None` is the documented "unlimited" spelling (`…MAX_CONNECTION_SECS=0`),
    /// and it must leave the socket untouched rather than closing it at once.
    #[tokio::test(start_paused = true)]
    async fn an_unbounded_socket_runs_to_its_own_end() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let closes = async move {
            tokio::time::sleep(Duration::from_secs(86_400)).await;
            flag.store(true, Ordering::SeqCst);
        };

        bounded_by(closes, None).await;

        assert!(done.load(Ordering::SeqCst), "no ceiling, no early close");
    }
}
