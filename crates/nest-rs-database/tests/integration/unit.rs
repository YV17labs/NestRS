//! `run_unit` against bindings written the way a driver writes one: the body
//! runs inside the unit the binding opened, the binding settles on the edge's
//! own outcome, and what it could not honour replaces a success.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use nest_rs_core::operation_log::{Kind, Unit};
use nest_rs_core::{Continuation, RequestContinuation};
use nest_rs_database::{
    Executor, ExecutorScope, OpenUnit, Settlement, TransactionPolicy, Unhonoured, UnitOfWork,
    UnitSpec, current_executor, current_executor_scope, current_unit_of_work, run_unit,
};

const MESSAGE: Unit =
    nest_rs_core::__private::declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message");

fn spec(scope: ExecutorScope) -> UnitSpec {
    UnitSpec::new(MESSAGE, scope, TransactionPolicy::PerUnit)
}

/// The handle a stub unit installs, naming the policy it was opened under.
struct Handle(TransactionPolicy);

impl Executor for Handle {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// What a stub binding saw: every unit it opened, and how each was settled.
#[derive(Default)]
struct Seen {
    opened: Vec<(&'static str, ExecutorScope, TransactionPolicy)>,
    settled: Vec<bool>,
}

/// A binding that records what it is asked, and settles with `answer`.
struct Stub {
    seen: Arc<Mutex<Seen>>,
    answer: Settlement,
}

impl Stub {
    fn answering(answer: Settlement) -> (Arc<dyn UnitOfWork>, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stub = Self {
            seen: Arc::clone(&seen),
            answer,
        };
        (Arc::new(stub), seen)
    }
}

struct StubUnit {
    policy: TransactionPolicy,
    seen: Arc<Mutex<Seen>>,
    answer: Settlement,
    /// Every handle this unit gave out, so its settle can tell one still held.
    handed: Mutex<Vec<Weak<Handle>>>,
}

impl UnitOfWork for Stub {
    fn open(&self, spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .opened
            .push((spec.unit.name(), spec.scope, spec.policy));
        Ok(Box::new(StubUnit {
            policy: spec.policy,
            seen: Arc::clone(&self.seen),
            answer: self.answer,
            handed: Mutex::new(Vec::new()),
        }))
    }
}

impl OpenUnit for StubUnit {
    fn executor(&self) -> Arc<dyn Executor> {
        let handle = Arc::new(Handle(self.policy));
        self.handed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::downgrade(&handle));
        handle
    }

    fn settle(self: Box<Self>, ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        // What a driver's escape check reads: a handle the body still held
        // would be one that escaped the unit.
        assert!(
            self.handed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .all(|handle| handle.upgrade().is_none()),
            "the body and its install are dropped before the unit settles",
        );
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .settled
            .push(ok);
        let answer = self.answer;
        Box::pin(async move { answer })
    }
}

fn seen(seen: &Arc<Mutex<Seen>>) -> std::sync::MutexGuard<'_, Seen> {
    seen.lock().unwrap_or_else(PoisonError::into_inner)
}

#[tokio::test]
async fn with_no_binding_the_body_runs_bare() {
    let ran: Result<(), ()> = run_unit(
        None,
        spec(ExecutorScope::Request),
        async {
            assert!(current_executor().is_none(), "nothing installs an executor");
            assert!(current_unit_of_work().is_none());
            Ok(())
        },
        Result::is_ok,
        |_| Err(()),
    )
    .await;
    assert_eq!(ran, Ok(()));
}

#[tokio::test]
async fn the_body_runs_on_the_units_executor_under_the_specs_scope() {
    for scope in [ExecutorScope::Request, ExecutorScope::Job] {
        let (binding, _) = Stub::answering(Settlement::Settled);
        let installed = run_unit(
            Some(&binding),
            spec(scope),
            async {
                (
                    current_executor().is_some_and(|executor| {
                        executor
                            .as_any()
                            .downcast_ref::<Handle>()
                            .is_some_and(|handle| handle.0 == TransactionPolicy::PerUnit)
                    }),
                    current_executor_scope(),
                )
            },
            |_| true,
            |_| (false, None),
        )
        .await;
        assert_eq!(installed, (true, Some(scope)));
        assert!(
            current_executor().is_none(),
            "the unit's executor ends with it"
        );
    }
}

#[tokio::test]
async fn the_binding_settles_on_the_edges_own_outcome() {
    let (binding, record) = Stub::answering(Settlement::Settled);
    for outcome in [Ok(1u8), Err("the handler failed")] {
        let answered = run_unit(
            Some(&binding),
            spec(ExecutorScope::Request),
            async move { outcome },
            Result::is_ok,
            |_| Err("unhonoured"),
        )
        .await;
        assert_eq!(
            answered, outcome,
            "a settled unit answers what its body did"
        );
    }
    assert_eq!(seen(&record).settled, [true, false]);
    assert_eq!(
        seen(&record).opened,
        [
            (
                "ws.message",
                ExecutorScope::Request,
                TransactionPolicy::PerUnit
            ),
            (
                "ws.message",
                ExecutorScope::Request,
                TransactionPolicy::PerUnit
            ),
        ],
        "one unit per run, opened as the spec declares it",
    );
}

#[tokio::test]
async fn a_success_the_binding_could_not_honour_becomes_the_edges_failure() {
    let why = Unhonoured::retryable("the unit's transaction could not be committed");
    let (binding, _) = Stub::answering(Settlement::Unhonoured(why));
    let answered = run_unit(
        Some(&binding),
        spec(ExecutorScope::Job),
        async { Ok::<_, Unhonoured>("the body's own success") },
        Result::is_ok,
        Err,
    )
    .await;
    assert_eq!(answered, Err(why));
}

#[tokio::test]
async fn a_failure_stands_whatever_the_binding_answers() {
    let (binding, _) = Stub::answering(Settlement::Unhonoured(Unhonoured::deterministic(
        "rolled back",
    )));
    let answered = run_unit(
        Some(&binding),
        spec(ExecutorScope::Request),
        async { Err::<(), _>("the handler's own failure") },
        Result::is_ok,
        |_| Err("unhonoured"),
    )
    .await;
    assert_eq!(answered, Err("the handler's own failure"));
}

/// A binding refusing what the spec asks — a policy it does not support.
struct Refusing;

impl UnitOfWork for Refusing {
    fn open(&self, _: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        Err(Unhonoured::deterministic(
            "this driver does not support the unit's transaction policy",
        ))
    }
}

#[tokio::test]
async fn a_unit_the_binding_refuses_fails_without_running_its_body() {
    let binding: Arc<dyn UnitOfWork> = Arc::new(Refusing);
    let ran = AtomicBool::new(false);
    let answered = run_unit(
        Some(&binding),
        spec(ExecutorScope::Job),
        async {
            ran.store(true, Ordering::SeqCst);
            Ok("the body's own success")
        },
        Result::is_ok,
        Err,
    )
    .await;
    assert_eq!(
        answered,
        Err(Unhonoured::deterministic(
            "this driver does not support the unit's transaction policy"
        )),
    );
    assert!(!ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn the_binding_is_ambient_inside_the_body_and_nowhere_else() {
    let (binding, _) = Stub::answering(Settlement::Settled);
    assert!(current_unit_of_work().is_none());
    let inside = run_unit(
        Some(&binding),
        spec(ExecutorScope::Request),
        async { current_unit_of_work() },
        |_| true,
        |_| None,
    )
    .await;
    assert!(inside.is_some_and(|seen| Arc::ptr_eq(&seen, &binding)));
    assert!(current_unit_of_work().is_none());
}

#[tokio::test]
async fn the_body_and_its_executor_are_gone_before_the_unit_settles() {
    let (binding, record) = Stub::answering(Settlement::Settled);
    run_unit(
        Some(&binding),
        spec(ExecutorScope::Request),
        async {
            let held = current_executor().expect("the unit's executor");
            tokio::task::yield_now().await;
            drop(held);
        },
        |()| true,
        |_| (),
    )
    .await;
    assert_eq!(seen(&record).settled, [true]);
}

#[tokio::test]
async fn a_continuation_that_outlives_the_unit_runs_on_a_pool_unit_of_the_same_binding() {
    let (binding, record) = Stub::answering(Settlement::Settled);
    let carried = run_unit(
        Some(&binding),
        spec(ExecutorScope::Request),
        async { RequestContinuation::capture(Continuation::Body) },
        |_| true,
        |_| None,
    )
    .await
    .expect("the unit carries its binding into its body");

    let (policy, scope, bound) = carried
        .scope(async {
            (
                current_executor().and_then(|executor| {
                    executor
                        .as_any()
                        .downcast_ref::<Handle>()
                        .map(|handle| handle.0)
                }),
                current_executor_scope(),
                current_unit_of_work().is_some_and(|seen| Arc::ptr_eq(&seen, &binding)),
            )
        })
        .await;
    assert_eq!(policy, Some(TransactionPolicy::Pool));
    assert_eq!(scope, Some(ExecutorScope::Request));
    assert!(bound, "the continuation still answers the binding");
    assert_eq!(
        seen(&record).opened.last(),
        Some(&(
            "ws.message",
            ExecutorScope::Request,
            TransactionPolicy::Pool
        )),
    );
}

#[tokio::test]
async fn a_continuation_that_opens_its_own_unit_carries_no_binding() {
    let (binding, _) = Stub::answering(Settlement::Settled);
    let carried = run_unit(
        Some(&binding),
        spec(ExecutorScope::Request),
        async { RequestContinuation::capture(Continuation::Task) },
        |_| true,
        |_| None,
    )
    .await;
    let crossed = match carried {
        Some(carried) => {
            carried
                .scope(async {
                    (
                        current_executor().is_some(),
                        current_unit_of_work().is_some(),
                    )
                })
                .await
        }
        None => (false, false),
    };
    assert_eq!(crossed, (false, false));
}

#[test]
fn running_a_unit_holds_its_body_once() {
    let heavy = || async {
        let held = [0u8; 32 * 1024];
        tokio::task::yield_now().await;
        std::hint::black_box(&held);
    };
    let alone = std::mem::size_of_val(&heavy());
    let (binding, _) = Stub::answering(Settlement::Settled);
    let size = std::mem::size_of_val(&run_unit(
        Some(&binding),
        spec(ExecutorScope::Request),
        heavy(),
        |()| true,
        |_| (),
    ));
    assert!(
        size < alone + 1024,
        "run_unit weighs {size} bytes around a {alone}-byte unit"
    );
}
