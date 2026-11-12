//! `TestApp::unit` and `system_unit`: a body runs in one unit the app's bound
//! `UnitOfWork` opens — as a request granted the ability given, or as system
//! work — and the unit settles on the body's own outcome.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nest_rs_authz::{Ability, AbilityBuilder, current_ability};
use nest_rs_core::module;
use nest_rs_database::{
    Executor, ExecutorScope, OpenUnit, Settlement, TransactionPolicy, Unhonoured, UnitOfWork,
    UnitSpec, current_executor_scope,
};
use nest_rs_testing::TestApp;

/// Every unit the binding opened, as `(name, scope, policy)`, and how each
/// settled.
#[derive(Default)]
struct Seen {
    opened: Vec<(&'static str, ExecutorScope, TransactionPolicy)>,
    settled: Vec<bool>,
}

static SEEN: Mutex<Seen> = Mutex::new(Seen {
    opened: Vec::new(),
    settled: Vec::new(),
});

fn seen() -> std::sync::MutexGuard<'static, Seen> {
    SEEN.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Handle;

impl Executor for Handle {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A binding that records what it is asked and settles everything.
struct Recording;

struct RecordedUnit;

impl UnitOfWork for Recording {
    fn open(&self, spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        seen()
            .opened
            .push((spec.unit.name(), spec.scope, spec.policy));
        Ok(Box::new(RecordedUnit))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(16)
    }
}

impl OpenUnit for RecordedUnit {
    fn executor(&self) -> Arc<dyn Executor> {
        Arc::new(Handle)
    }

    fn settle(self: Box<Self>, ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        seen().settled.push(ok);
        Box::pin(std::future::ready(Settlement::Settled))
    }
}

#[module]
struct DataModule;

fn reader() -> Arc<Ability> {
    Arc::new(
        AbilityBuilder::new()
            .build()
            .expect("an empty ability builds"),
    )
}

/// What the body read of the unit it ran in.
async fn ambient() -> Result<(Option<ExecutorScope>, Option<Arc<Ability>>), ()> {
    Ok((current_executor_scope(), current_ability()))
}

// One test, so the recorded units are this test's alone.
#[tokio::test]
async fn a_body_runs_in_one_unit_of_the_binding_as_a_request_or_as_system_work() {
    let app = TestApp::builder()
        .module::<DataModule>()
        .provide_dyn::<dyn UnitOfWork>(Arc::new(Recording))
        .build()
        .await
        .expect("the app boots on its binding");
    let granted = reader();

    let (scope, ability) = app
        .unit(Arc::clone(&granted), ambient)
        .await
        .expect("the body answers");
    assert_eq!(scope, Some(ExecutorScope::Request));
    assert!(
        ability.is_some_and(|ability| Arc::ptr_eq(&ability, &granted)),
        "the body reads the ability it was granted",
    );

    let (scope, ability) = app.system_unit(ambient).await.expect("the body answers");
    assert_eq!(scope, Some(ExecutorScope::Job));
    assert!(ability.is_none(), "system work carries no ability");

    let refused: Result<(), &str> = app
        .unit(reader(), || async { Err("the service refused") })
        .await;
    assert_eq!(refused, Err("the service refused"));

    let seen = seen();
    assert_eq!(
        seen.opened,
        [
            (
                "testing.unit",
                ExecutorScope::Request,
                TransactionPolicy::PerUnit
            ),
            (
                "testing.unit",
                ExecutorScope::Job,
                TransactionPolicy::PerUnit
            ),
            (
                "testing.unit",
                ExecutorScope::Request,
                TransactionPolicy::PerUnit
            ),
        ],
        "each body ran in one unit the binding opened",
    );
    assert_eq!(
        seen.settled,
        [true, true, false],
        "a unit commits on its body's `Ok` and rolls back on its `Err`",
    );
}

#[tokio::test]
#[should_panic(expected = "the app binds no unit of work")]
async fn an_app_binding_no_unit_of_work_runs_no_unit() {
    let app = TestApp::for_module::<DataModule>()
        .await
        .expect("the app boots");
    let _ = app.system_unit(ambient).await;
}
