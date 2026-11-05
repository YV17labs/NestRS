//! `with_ability` inside the units an app's binding opens: it narrows the
//! ability a request holds, and scopes a job's unscoped reads; a job unit
//! opened, or a capture re-entered, inside a request reads as that request.

use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_authz::{Ability, AbilityBuilder, Action, current_ability, with_ability};
use nest_rs_core::{Continuation, RequestContinuation, module};
use nest_rs_database::__private::{with_job_executor, with_request_executor};
use nest_rs_database::{
    Executor, ExecutorScope, OpenUnit, Settlement, TransactionPolicy, Unhonoured, UnitOfWork,
    UnitSpec, current_executor_scope, current_unit_of_work, run_unit,
};
use nest_rs_testing::TestApp;

use crate::parent;

struct Handle;

impl Executor for Handle {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A binding whose every unit commits.
struct Committing;

struct CommittingUnit;

impl UnitOfWork for Committing {
    fn open(&self, _spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        Ok(Box::new(CommittingUnit))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(16)
    }
}

impl OpenUnit for CommittingUnit {
    fn executor(&self) -> Arc<dyn Executor> {
        Arc::new(Handle)
    }

    fn settle(self: Box<Self>, _ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        Box::pin(std::future::ready(Settlement::Settled))
    }
}

#[module]
struct DataModule;

async fn app() -> TestApp {
    TestApp::builder()
        .module::<DataModule>()
        .provide_dyn::<dyn UnitOfWork>(Arc::new(Committing))
        .build()
        .await
        .expect("the app boots on its binding")
}

fn granting(actions: &[Action]) -> Arc<Ability> {
    let mut ab = AbilityBuilder::new();
    for action in actions {
        ab.can(*action, parent::Entity);
    }
    Arc::new(ab.build().expect("a valid rule set"))
}

/// Which of `actions` the body's reads run allowed to take on parents, or
/// `None` when they run under no ability.
async fn allowed(actions: &[Action]) -> Result<Option<Vec<Action>>, ()> {
    Ok(current_ability().map(|ability| {
        actions
            .iter()
            .copied()
            .filter(|action| ability.can_class(*action, TypeId::of::<parent::Entity>()))
            .collect()
    }))
}

#[tokio::test]
async fn a_request_handed_a_wider_ability_runs_under_the_one_it_holds() {
    let app = app().await;
    let both = [Action::Read, Action::Delete];

    let read = app
        .unit(granting(&[Action::Read]), || {
            with_ability(granting(&both), allowed(&both))
        })
        .await
        .expect("the body answers");

    assert_eq!(read, Some(vec![Action::Read]));
}

#[tokio::test]
async fn a_job_acting_for_a_principal_reads_under_its_ability() {
    let app = app().await;
    let both = [Action::Read, Action::Delete];

    let read = app
        .system_unit(|| with_ability(granting(&[Action::Read]), allowed(&both)))
        .await
        .expect("the body answers");

    assert_eq!(read, Some(vec![Action::Read]));
}

#[tokio::test]
async fn a_request_unit_handed_a_visitor_reads_what_the_visitor_may() {
    let app = app().await;
    let mut visitor = AbilityBuilder::new();
    visitor.can(Action::Read, parent::Entity);
    let visitor = Arc::new(visitor.build_visitor().expect("a valid rule set"));

    let read = app
        .unit(visitor, || allowed(&[Action::Read, Action::Delete]))
        .await
        .expect("the body answers");

    assert_eq!(read, Some(vec![Action::Read]));
}

#[tokio::test]
async fn a_job_acting_for_a_visitor_reads_under_its_ability() {
    let app = app().await;
    let mut visitor = AbilityBuilder::new();
    visitor.can(Action::Read, parent::Entity);
    let visitor = Arc::new(visitor.build_visitor().expect("a valid rule set"));

    let read = app
        .system_unit(|| with_ability(visitor, allowed(&[Action::Read, Action::Delete])))
        .await
        .expect("the body answers");

    assert_eq!(read, Some(vec![Action::Read]));
}

/// A unit a product's feature declares and opens itself.
const EXPORT: nest_rs_core::operation_log::Unit = nest_rs_core::__private::declare_unit(
    "features",
    "features",
    nest_rs_core::operation_log::Kind::Internal,
    "features.export",
);

/// The scope a job unit `binding` opens in place runs under, and what
/// `with_ability` lets its reads take there.
async fn a_job_unit_reading_under(
    binding: Arc<dyn UnitOfWork>,
    actions: &[Action],
) -> Result<(Option<ExecutorScope>, Option<Vec<Action>>), ()> {
    run_unit(
        Some(&binding),
        UnitSpec::new(EXPORT, ExecutorScope::Job, TransactionPolicy::Pool),
        async {
            let read = with_ability(granting(actions), allowed(actions)).await?;
            Ok((current_executor_scope(), read))
        },
        Result::is_ok,
        |_| Err(()),
    )
    .await
}

#[tokio::test]
async fn a_job_unit_opened_inside_a_request_reads_as_the_request_does() {
    let app = app().await;
    let both = [Action::Read, Action::Delete];

    let authorized = app
        .unit(granting(&[Action::Read]), || async {
            let binding = current_unit_of_work().expect("the app's binding opened the unit");
            a_job_unit_reading_under(binding, &both).await
        })
        .await
        .expect("the body answers");
    let unauthorized = with_request_executor(
        Arc::new(Handle),
        a_job_unit_reading_under(Arc::new(Committing), &both),
    )
    .await
    .expect("the body answers");

    assert_eq!(
        authorized,
        (Some(ExecutorScope::Request), Some(vec![Action::Read]))
    );
    assert_eq!(
        unauthorized,
        (Some(ExecutorScope::Request), None),
        "a request no ability guard authorized stays closed",
    );
}

#[tokio::test]
async fn a_capture_re_entered_in_another_callers_request_reads_under_what_both_allow() {
    let app = app().await;
    let both = [Action::Read, Action::Delete];

    let captured = app
        .unit(granting(&both), || async {
            Ok::<_, ()>(RequestContinuation::capture(Continuation::Commit))
        })
        .await
        .expect("the body answers")
        .expect("a unit runs");
    let read = app
        .unit(granting(&[Action::Read]), || captured.scope(allowed(&both)))
        .await
        .expect("the body answers");

    assert_eq!(read, Some(vec![Action::Read]));
}

#[tokio::test]
async fn a_jobs_capture_re_entered_inside_a_request_reads_as_the_request_does() {
    let both = [Action::Read, Action::Delete];

    let captured = with_job_executor(Arc::new(Handle), async {
        RequestContinuation::capture(Continuation::Commit)
    })
    .await
    .expect("a unit runs");
    let seen = with_request_executor(
        Arc::new(Handle),
        captured.scope(async {
            let read = with_ability(granting(&both), allowed(&both)).await;
            (current_executor_scope(), read)
        }),
    )
    .await;

    assert_eq!(
        seen,
        (Some(ExecutorScope::Request), Ok(None)),
        "a request no ability guard authorized stays closed",
    );
}
