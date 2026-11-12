//! A unit of work a test runs through the app's bound `UnitOfWork` (feature
//! `database`), as a request granted an ability (feature `authz`) or as system
//! work.

use std::future::Future;

use nest_rs_core::Container;
use nest_rs_core::operation_log::Unit;
use nest_rs_database::{
    BACKEND_REMEDY, ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit,
};

/// The unit a test runs, as the driver's events name it.
const TEST_UNIT: Unit = nest_rs_core::unit!("testing.unit", target: TARGET, kind: Internal);

/// What this crate's units file their events on.
const TARGET: &str = "nest_rs::testing";

/// Run `body` as one unit of work of the binding `container` holds, under
/// `scope`: one transaction, committed on `Ok` and rolled back on `Err`.
pub(crate) async fn run<T, E, Fut>(
    container: &Container,
    scope: ExecutorScope,
    body: Fut,
) -> Result<T, E>
where
    T: Send,
    E: Send,
    Fut: Future<Output = Result<T, E>> + Send,
{
    let binding = container
        .get_dyn::<dyn UnitOfWork>()
        .unwrap_or_else(|| panic!("the app binds no unit of work. {BACKEND_REMEDY}"));
    run_unit(
        Some(&binding),
        UnitSpec::new(TEST_UNIT, scope, TransactionPolicy::PerUnit),
        body,
        Result::is_ok,
        |unhonoured| panic!("the unit's driver could not honour it: {unhonoured}"),
    )
    .await
}
