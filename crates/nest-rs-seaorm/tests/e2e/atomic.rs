//! `nest_rs_database::atomic` over SeaORM: a savepoint inside a unit's
//! transaction, a transaction of its own on the pool, and an enclosing unit
//! that waits for it and keeps what it wrote beside it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nest_rs_core::module;
use nest_rs_database::{AtomicError, atomic};
use nest_rs_seaorm::{
    DbOutcome, Executor, FinalizeOutcome, LazyTransaction, current_executor, with_executor,
    with_request_executor,
};
use sea_orm::prelude::Uuid;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};

#[derive(Debug)]
enum Failure {
    Db(DbErr),
    Atomic(AtomicError),
    Refused,
}

impl From<DbErr> for Failure {
    fn from(err: DbErr) -> Self {
        Self::Db(err)
    }
}

impl From<AtomicError> for Failure {
    fn from(err: AtomicError) -> Self {
        Self::Atomic(err)
    }
}

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "atomic_probe",
        "CREATE TABLE IF NOT EXISTS atomic_probe (id UUID PRIMARY KEY);",
    )
    .await;
    conn
}

/// Insert `id` on the ambient executor: the block's inside one, the unit's
/// outside.
async fn insert(id: Uuid) -> Result<(), DbErr> {
    let executor = current_executor().ok_or_else(|| DbErr::Custom("no executor".into()))?;
    insert_on(&executor, id).await
}

async fn insert_on(executor: &Executor, id: Uuid) -> Result<(), DbErr> {
    executor
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO atomic_probe (id) VALUES ($1)",
            [id.into()],
        ))
        .await
        .map(|_| ())
}

async fn stored(conn: &DatabaseConnection, id: Uuid) -> bool {
    conn.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT id FROM atomic_probe WHERE id = $1",
        [id.into()],
    ))
    .await
    .expect("the probe reads")
    .is_some()
}

fn boundary(conn: &DatabaseConnection) -> Arc<LazyTransaction> {
    Arc::new(LazyTransaction::new(conn.clone(), "test"))
}

async fn committed(lazy: Arc<LazyTransaction>) {
    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Committed),
        "the unit commits what it wrote beside the block, got {outcome:?}",
    );
}

#[tokio::test]
async fn a_unique_violation_inside_a_block_leaves_the_request_free_to_write_and_commit() {
    let conn = db().await;
    let (taken, after) = (Uuid::now_v7(), Uuid::now_v7());
    insert_on(&Executor::Pool(conn.clone()), taken)
        .await
        .expect("the clashing row is seeded");

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let clash = atomic(async { Ok::<_, Failure>(insert(taken).await?) }).await;
        assert!(
            matches!(&clash, Err(Failure::Db(err)) if DbOutcome::of(err) == DbOutcome::Conflict),
            "{clash:?}"
        );
        insert(after)
            .await
            .expect("the request writes on after the block rolled back");
    })
    .await;
    committed(lazy).await;

    assert!(stored(&conn, after).await);
}

#[tokio::test]
async fn a_block_returning_err_after_writing_leaves_no_row() {
    let conn = db().await;
    let written = Uuid::now_v7();

    let lazy = boundary(&conn);
    let outcome = with_request_executor(
        Executor::Lazy(Arc::clone(&lazy)),
        atomic(async {
            insert(written).await?;
            Err::<(), _>(Failure::Refused)
        }),
    )
    .await;
    assert!(matches!(outcome, Err(Failure::Refused)), "{outcome:?}");
    committed(lazy).await;

    assert!(!stored(&conn, written).await);
}

#[tokio::test]
async fn a_block_on_the_pool_commits_on_its_own() {
    let conn = db().await;
    let written = Uuid::now_v7();

    let outcome = with_request_executor(
        Executor::Pool(conn.clone()),
        atomic(async { Ok::<_, Failure>(insert(written).await?) }),
    )
    .await;

    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(
        stored(&conn, written).await,
        "visible once the block returned"
    );
}

#[tokio::test]
async fn a_block_on_the_pool_returning_err_leaves_no_row() {
    let conn = db().await;
    let written = Uuid::now_v7();

    let outcome = with_request_executor(
        Executor::Pool(conn.clone()),
        atomic(async {
            insert(written).await?;
            Err::<(), _>(Failure::Refused)
        }),
    )
    .await;

    assert!(matches!(outcome, Err(Failure::Refused)), "{outcome:?}");
    assert!(!stored(&conn, written).await);
}

#[tokio::test]
async fn a_nested_block_rolls_back_alone() {
    let conn = db().await;
    let (kept, dropped) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = boundary(&conn);
    let outcome = with_request_executor(
        Executor::Lazy(Arc::clone(&lazy)),
        atomic(async {
            insert(kept).await?;
            let inner = atomic(async {
                insert(dropped).await?;
                Err::<(), _>(Failure::Refused)
            })
            .await;
            assert!(matches!(inner, Err(Failure::Refused)), "{inner:?}");
            Ok::<_, Failure>(())
        }),
    )
    .await;
    assert!(outcome.is_ok(), "{outcome:?}");
    committed(lazy).await;

    assert!(stored(&conn, kept).await);
    assert!(!stored(&conn, dropped).await);
}

#[tokio::test]
async fn a_block_returning_ok_over_a_swallowed_failure_is_poisoned_and_keeps_nothing() {
    let conn = db().await;
    let (taken, written) = (Uuid::now_v7(), Uuid::now_v7());
    insert_on(&Executor::Pool(conn.clone()), taken)
        .await
        .expect("the clashing row is seeded");

    let lazy = boundary(&conn);
    let outcome = with_request_executor(
        Executor::Lazy(Arc::clone(&lazy)),
        atomic(async {
            insert(written).await?;
            let _ = insert(taken).await;
            Ok::<_, Failure>(())
        }),
    )
    .await;
    assert!(
        matches!(
            outcome,
            Err(Failure::Atomic(AtomicError::Poisoned { retryable: false }))
        ),
        "a unique violation would fail again, got {outcome:?}",
    );
    committed(lazy).await;

    assert!(!stored(&conn, written).await);
}

#[tokio::test]
async fn the_unit_waits_for_an_open_block_and_keeps_its_own_write() {
    let conn = db().await;
    let (inside, beside) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let (block, parent) = tokio::join!(
            atomic(async {
                insert(inside).await?;
                tokio::time::sleep(Duration::from_millis(50)).await;
                Err::<(), _>(Failure::Refused)
            }),
            insert(beside),
        );
        assert!(matches!(block, Err(Failure::Refused)), "{block:?}");
        parent.expect("the unit's own write lands once the block settled");
    })
    .await;
    committed(lazy).await;

    assert!(!stored(&conn, inside).await);
    assert!(
        stored(&conn, beside).await,
        "the unit waited: its write is not inside the rolled-back savepoint",
    );
}

#[tokio::test]
async fn a_panicking_block_rolls_back_and_the_request_continues() {
    let conn = db().await;
    let (inside, after) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unwound = nest_rs_core::panic::contain(atomic(async {
            insert(inside).await?;
            explode()
        }))
        .await;
        std::panic::set_hook(previous);
        assert!(unwound.is_err(), "the panic unwinds past the block");

        insert(after)
            .await
            .expect("the request writes on after the block rolled back");
    })
    .await;
    committed(lazy).await;

    assert!(!stored(&conn, inside).await);
    assert!(stored(&conn, after).await);
}

fn explode() -> Result<(), Failure> {
    panic!("inside the block")
}

#[tokio::test]
async fn a_statement_on_the_unit_from_inside_its_block_is_refused_rather_than_waited_for() {
    let conn = db().await;
    let written = Uuid::now_v7();

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let unit = current_executor().expect("the unit's executor");
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            atomic(async {
                insert(Uuid::now_v7()).await?;
                Ok::<_, Failure>(insert_on(&unit, written).await?)
            }),
        )
        .await
        .expect("the block never waits for itself");
        assert!(
            matches!(&outcome, Err(Failure::Db(DbErr::Custom(said))) if said.contains("atomic blocks")),
            "{outcome:?}"
        );
    })
    .await;
    committed(lazy).await;

    assert!(!stored(&conn, written).await);
}

#[tokio::test]
async fn a_unit_statement_issued_while_its_block_waits_to_open_is_refused() {
    let conn = db().await;
    let (beside, inside, refused) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let unit = current_executor().expect("the unit's executor");
        let (parent, block) = tokio::join!(
            insert(beside),
            tokio::time::timeout(
                Duration::from_secs(10),
                atomic(async {
                    let (own, on_unit) = tokio::join!(insert(inside), insert_on(&unit, refused));
                    own?;
                    Ok::<_, Failure>(on_unit)
                }),
            ),
        );
        parent.expect("the unit's own write lands");
        let on_unit = block
            .expect("the block never waits for itself")
            .expect("the block keeps its own write");
        assert!(
            matches!(&on_unit, Err(DbErr::Custom(said)) if said.contains("atomic blocks")),
            "{on_unit:?}"
        );
    })
    .await;
    committed(lazy).await;

    assert!(stored(&conn, beside).await);
    assert!(stored(&conn, inside).await);
    assert!(!stored(&conn, refused).await);
}

#[tokio::test]
async fn a_block_whose_executor_escapes_keeps_nothing_and_refuses_the_late_write() {
    let conn = db().await;
    let (inside, late) = (Uuid::now_v7(), Uuid::now_v7());
    let escaped: Arc<Mutex<Option<Executor>>> = Arc::default();

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let outcome = atomic(async {
            insert(inside).await?;
            *escaped.lock().unwrap_or_else(PoisonError::into_inner) = current_executor();
            Ok::<_, Failure>(())
        })
        .await;
        assert!(
            matches!(outcome, Err(Failure::Atomic(AtomicError::Escaped))),
            "{outcome:?}"
        );
        let handle = escaped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("the escaped handle");
        assert!(
            insert_on(&handle, late).await.is_err(),
            "a handle that outlived its block writes nothing"
        );
    })
    .await;
    committed(lazy).await;

    assert!(!stored(&conn, inside).await);
    assert!(!stored(&conn, late).await);
}

#[tokio::test]
async fn a_block_dropped_before_it_settles_leaves_its_unit_unable_to_commit() {
    let conn = db().await;
    let (before, inside) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        insert(before).await.expect("the unit writes first");
        let (wrote, written) = tokio::sync::oneshot::channel();
        let block = atomic(async {
            insert(inside).await?;
            let _ = wrote.send(());
            std::future::pending::<()>().await;
            Ok::<_, Failure>(())
        });
        tokio::select! {
            _ = block => panic!("the block never ends on its own"),
            _ = written => {}
        }
    })
    .await;

    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Poisoned { retryable: false }),
        "{outcome:?}"
    );
    assert!(!stored(&conn, before).await);
    assert!(!stored(&conn, inside).await);
}

#[tokio::test]
async fn inside_a_callers_own_transaction_a_block_nests_and_the_caller_commits() {
    let conn = db().await;
    let (kept, dropped, beside) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    let txn = Arc::new(conn.begin().await.expect("the caller opens its own"));
    with_executor(Executor::Txn(Arc::clone(&txn)), async {
        let released = atomic(async { Ok::<_, Failure>(insert(kept).await?) }).await;
        assert!(released.is_ok(), "{released:?}");
        let rolled_back = atomic(async {
            insert(dropped).await?;
            Err::<(), _>(Failure::Refused)
        })
        .await;
        assert!(
            matches!(rolled_back, Err(Failure::Refused)),
            "{rolled_back:?}"
        );
        insert(beside).await.expect("the caller writes on");
    })
    .await;
    Arc::try_unwrap(txn)
        .expect("no handle outlived the scope")
        .commit()
        .await
        .expect("the caller commits");

    assert!(stored(&conn, kept).await);
    assert!(!stored(&conn, dropped).await);
    assert!(stored(&conn, beside).await);
}

/// What an event's listener saw, in order.
#[nest_rs_core::injectable]
#[derive(Default)]
struct Heard {
    events: Mutex<Vec<&'static str>>,
}

#[nest_rs_core::injectable]
struct HeardListeners {
    #[inject]
    heard: Arc<Heard>,
}

#[derive(Clone)]
struct Announced(&'static str);

#[nest_rs_events::listeners]
impl HeardListeners {
    #[on_event]
    async fn on_announced(&self, event: Announced) {
        self.heard
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event.0);
    }
}

#[module(
    imports = [nest_rs_events::EventsModule],
    providers = [Heard, HeardListeners],
)]
struct HeardModule;

#[tokio::test]
async fn an_event_emitted_in_a_block_waits_for_the_outer_commit_and_dies_with_a_rollback() {
    let conn = db().await;
    let app = nest_rs_core::App::new::<HeardModule>().expect("boots");
    let bus = app
        .container()
        .get::<nest_rs_events::EventBus>()
        .expect("the bus is provided");
    let heard = app.container().get::<Heard>().expect("the probe");
    let heard_now = || {
        heard
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    };

    let lazy = boundary(&conn);
    with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let released = atomic(async {
            insert(Uuid::now_v7()).await?;
            bus.emit(Announced("released")).await;
            Ok::<_, Failure>(())
        })
        .await;
        assert!(released.is_ok(), "{released:?}");
        let rolled_back = atomic(async {
            insert(Uuid::now_v7()).await?;
            bus.emit(Announced("rolled back")).await;
            Err::<(), _>(Failure::Refused)
        })
        .await;
        assert!(
            matches!(rolled_back, Err(Failure::Refused)),
            "{rolled_back:?}"
        );
    })
    .await;
    assert!(
        heard_now().is_empty(),
        "nothing is heard before the unit commits"
    );

    committed(lazy).await;
    nest_rs_testing::wait_until(Duration::from_secs(5), || !heard_now().is_empty()).await;
    assert_eq!(heard_now(), ["released"]);
}
