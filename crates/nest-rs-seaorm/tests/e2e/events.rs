//! A listener's unit of work over `SeaOrmUnitOfWork`, held for its emitter's
//! commit: its writes are one transaction settled on its answer, an event it
//! emits waits for that commit, a pool listener's writes stand on their own,
//! and a listener cut at its timeout rolls back and gives its connection back.
//! A task emitting off every unit reaches the same through the app's binding,
//! and a listener `#[resilient]` decorates keeps it: a run failed or refused
//! after its write leaves nothing committed.
//!
//! Each test runs over a one-connection pool and counts through it, so a count
//! waits for the connection the listener's settle gives back.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{App, injectable, module};
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_events::{EventBus, EventsModule, listeners};
use nest_rs_resilience::{
    ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
};
use nest_rs_seaorm::__private::current_executor;
use nest_rs_seaorm::{Repo, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, SeaOrmUnitOfWork};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};

/// A queue attempt's unit, the emitter's here.
const EMITTER: nest_rs_core::operation_log::Unit = nest_rs_core::__private::declare_unit(
    "nest-rs-queue",
    "nest_rs::queue",
    nest_rs_core::operation_log::Kind::Consumer,
    "queue.job",
);

/// A pool of one connection, so whatever holds it holds every later query.
async fn one_connection() -> Arc<DatabaseConnection> {
    let mut options = crate::harness::options(crate::harness::url());
    options.max_connections(1).min_connections(0);
    Arc::new(
        Database::connect(options)
            .await
            .unwrap_or_else(|err| panic!("connect to the suite's Postgres: {err}")),
    )
}

/// One row into `table`, on the listener's own executor.
async fn insert(table: &str) -> Result<(), sea_orm::DbErr> {
    current_executor()
        .expect("a listener runs on the executor of its unit")
        .execute_unprepared(&format!("INSERT INTO {table} DEFAULT VALUES"))
        .await
        .map(drop)
}

/// Emit `event` inside a unit of the binding over `conn`, as a queue attempt
/// would: the dispatch is held for the attempt's commit.
async fn emitted_in_a_unit<E: Clone + Send + 'static>(
    conn: &Arc<DatabaseConnection>,
    bus: &EventBus,
    event: E,
) {
    let binding: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(
        Arc::clone(conn),
        &SeaOrmConfig::default(),
    ));
    let spec = UnitSpec::new(EMITTER, ExecutorScope::Job, TransactionPolicy::PerUnit);
    run_unit(
        Some(&binding),
        spec,
        async {
            bus.emit(event).await;
            Ok::<(), ()>(())
        },
        Result::is_ok,
        |_| Err(()),
    )
    .await
    .expect("the emitter's unit commits");
}

fn bus(app: &App) -> Arc<EventBus> {
    app.container()
        .get::<EventBus>()
        .expect("the bus is provided")
}

#[derive(Clone)]
struct TwoRowsAsked {
    table: &'static str,
    fails: bool,
}

#[derive(Clone)]
struct OnThePoolAsked {
    table: &'static str,
}

#[injectable]
#[derive(Default)]
struct WritingListeners;

#[listeners]
impl WritingListeners {
    #[on_event]
    async fn writes_two_rows(&self, event: TwoRowsAsked) -> anyhow::Result<()> {
        insert(event.table).await?;
        insert(event.table).await?;
        anyhow::ensure!(!event.fails, "the reaction refused after writing");
        Ok(())
    }

    #[on_event(transactional = false)]
    async fn writes_on_the_pool(&self, event: OnThePoolAsked) -> anyhow::Result<()> {
        insert(event.table).await?;
        anyhow::bail!("the reaction refused after its first write")
    }
}

#[module(imports = [EventsModule], providers = [WritingListeners])]
struct WritingModule;

#[tokio::test]
async fn a_listener_answering_ok_commits_both_rows_and_one_answering_err_leaves_neither() {
    let conn = one_connection().await;
    let table = "listener_unit_two_rows";
    crate::harness::fresh_table(&conn, table).await;
    let app = App::builder()
        .module::<WritingModule>()
        .build()
        .await
        .expect("boots");

    emitted_in_a_unit(&conn, &bus(&app), TwoRowsAsked { table, fails: true }).await;
    assert_eq!(
        crate::harness::committed_rows(&*conn, table).await,
        0,
        "a listener that failed after two writes rolled both back",
    );

    emitted_in_a_unit(
        &conn,
        &bus(&app),
        TwoRowsAsked {
            table,
            fails: false,
        },
    )
    .await;
    assert_eq!(
        crate::harness::committed_rows(&*conn, table).await,
        2,
        "a listener answering Ok committed both writes"
    );
}

#[tokio::test]
async fn a_pool_listeners_first_write_stands_after_its_err() {
    let conn = one_connection().await;
    let table = "listener_unit_on_the_pool";
    crate::harness::fresh_table(&conn, table).await;
    let app = App::builder()
        .module::<WritingModule>()
        .build()
        .await
        .expect("boots");

    emitted_in_a_unit(&conn, &bus(&app), OnThePoolAsked { table }).await;

    assert_eq!(
        crate::harness::committed_rows(&*conn, table).await,
        1,
        "`transactional = false` commits each statement on its own",
    );
}

#[derive(Clone)]
struct Published {
    table: &'static str,
    fails: bool,
}

#[derive(Clone)]
struct Indexed {
    table: &'static str,
}

/// What the second listener of the chain saw committed when it ran.
#[injectable]
#[derive(Default)]
struct ChainProbe {
    seen: std::sync::Mutex<Vec<i64>>,
}

impl ChainProbe {
    fn seen(&self) -> Vec<i64> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[injectable]
struct ChainListeners {
    #[inject]
    bus: Arc<EventBus>,
    #[inject]
    probe: Arc<ChainProbe>,
}

#[listeners]
impl ChainListeners {
    #[on_event]
    async fn on_published(&self, event: Published) -> anyhow::Result<()> {
        insert(event.table).await?;
        self.bus.emit(Indexed { table: event.table }).await;
        anyhow::ensure!(!event.fails, "the reaction refused after emitting");
        Ok(())
    }

    #[on_event]
    async fn on_indexed(&self, event: Indexed) -> anyhow::Result<()> {
        let executor = current_executor().expect("a listener runs on an executor");
        let committed = crate::harness::committed_rows(&executor, event.table).await;
        self.probe
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(committed);
        Ok(())
    }
}

#[module(imports = [EventsModule], providers = [ChainProbe, ChainListeners])]
struct ChainModule;

#[tokio::test]
async fn an_event_a_listener_emits_dispatches_after_its_commit_and_never_on_its_failure() {
    let conn = one_connection().await;
    let table = "listener_unit_chain";
    crate::harness::fresh_table(&conn, table).await;
    let app = App::builder()
        .module::<ChainModule>()
        .build()
        .await
        .expect("boots");
    let probe = app
        .container()
        .get::<ChainProbe>()
        .expect("the probe is provided");

    emitted_in_a_unit(&conn, &bus(&app), Published { table, fails: true }).await;
    assert!(
        probe.seen().is_empty(),
        "the listener rolled back, so the fact it announced never happened",
    );
    assert_eq!(crate::harness::committed_rows(&*conn, table).await, 0);

    emitted_in_a_unit(
        &conn,
        &bus(&app),
        Published {
            table,
            fails: false,
        },
    )
    .await;
    assert_eq!(
        probe.seen(),
        [1],
        "the event it emitted ran once its row had committed",
    );
}

#[derive(Clone)]
struct StallAsked {
    table: &'static str,
}

#[injectable]
#[derive(Default)]
struct StallingListeners;

#[listeners]
impl StallingListeners {
    #[on_event(timeout = "1s")]
    async fn writes_then_stalls(&self, event: StallAsked) -> anyhow::Result<()> {
        insert(event.table).await?;
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(())
    }
}

#[module(imports = [EventsModule], providers = [StallingListeners])]
struct StallingModule;

#[tokio::test]
async fn a_listener_cut_at_its_timeout_leaves_no_row_and_gives_its_connection_back() {
    let conn = one_connection().await;
    let table = "listener_unit_cut";
    crate::harness::fresh_table(&conn, table).await;
    let app = App::builder()
        .module::<StallingModule>()
        .build()
        .await
        .expect("boots");

    let emitted = std::time::Instant::now();
    emitted_in_a_unit(&conn, &bus(&app), StallAsked { table }).await;
    let took = emitted.elapsed();

    assert!(
        took < Duration::from_secs(30),
        "cut at its 1 s timeout, not after its 30 s sleep, which a listener left uncut \
         takes whole: {took:?}",
    );
    assert_eq!(
        crate::harness::committed_rows(&*conn, table).await,
        0,
        "the cut listener's write rolled back, and the pool's one connection came back",
    );
}

mod off_every_unit {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "listener_unit_off_every_unit")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[derive(Clone)]
struct RowAsked {
    fails: bool,
}

#[injectable]
#[derive(Default)]
struct RepoListeners;

#[listeners]
impl RepoListeners {
    #[on_event]
    async fn writes_through_repo(&self, event: RowAsked) -> anyhow::Result<()> {
        Repo::<off_every_unit::Entity>::insert_unscoped(off_every_unit::ActiveModel::default())
            .await?;
        anyhow::ensure!(!event.fails, "the reaction refused after writing");
        Ok(())
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        EventsModule,
    ],
    providers = [RepoListeners],
)]
struct BoundModule;

/// `event`, emitted by a task holding no unit of its own.
async fn emitted_off_every_unit<E: Clone + Send + 'static>(bus: Arc<EventBus>, event: E) {
    let task = nest_rs_core::TaskContext::current().carry(async move { bus.emit(event).await });
    tokio::spawn(task).await.expect("the task ends");
}

#[tokio::test]
async fn a_listener_a_spawned_task_emits_to_settles_its_repo_write_on_its_answer() {
    let conn = crate::harness::connect().await;
    let table = "listener_unit_off_every_unit";
    crate::harness::fresh_table(&conn, table).await;
    let app = App::builder()
        .module::<BoundModule>()
        .build()
        .await
        .expect("boots");

    emitted_off_every_unit(bus(&app), RowAsked { fails: true }).await;
    assert_eq!(
        crate::harness::committed_rows(&conn, table).await,
        0,
        "its `Err` rolled its write back",
    );

    emitted_off_every_unit(bus(&app), RowAsked { fails: false }).await;
    assert_eq!(
        crate::harness::committed_rows(&conn, table).await,
        1,
        "its `Ok` committed its write",
    );
}

mod resilient_listener {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "listener_unit_resilient")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// What a decorated listener's call answers once it wrote its row.
#[derive(Clone, Copy, Debug)]
enum Then {
    /// The dependency answers: the row commits.
    Answers,
    /// The dependency fails: the run ends on its error.
    Fails,
    /// The dependency never answers: the policy gives the run up at the
    /// method's timeout, a refusal.
    Stalls,
}

#[derive(Clone)]
struct RefundAsked {
    then: Then,
}

#[derive(Clone)]
struct RestockAsked;

#[derive(Debug, thiserror::Error)]
enum Refund {
    #[error("the refund's row was refused")]
    Db(#[from] nest_rs_seaorm::RepoError),
    #[error("the payment provider refused the refund")]
    Refused,
}

impl nest_rs_core::ToProblem for Refund {
    fn to_problem(&self) -> Option<nest_rs_core::Problem> {
        None
    }
}

#[injectable]
struct ResilientListeners {
    #[inject]
    upstream: Arc<ResiliencePolicy>,
    attempts: std::sync::atomic::AtomicU32,
}

#[listeners]
impl ResilientListeners {
    /// One row through `Repo`, then the call the event says.
    #[on_event]
    #[resilient(upstream, timeout = "200ms")]
    async fn refund(&self, event: RefundAsked) -> Result<(), ResilienceError<Refund>> {
        Repo::<resilient_listener::Entity>::insert_unscoped(
            resilient_listener::ActiveModel::default(),
        )
        .await?;
        match event.then {
            Then::Answers => Ok(()),
            Then::Fails => Err(Refund::Refused),
            Then::Stalls => std::future::pending().await,
        }
    }

    /// One row through `Repo` each attempt, the first failing once it wrote
    /// its own.
    #[on_event]
    #[resilient(upstream, idempotent)]
    async fn restock(&self, _event: RestockAsked) -> Result<(), ResilienceError<Refund>> {
        Repo::<resilient_listener::Entity>::insert_unscoped(
            resilient_listener::ActiveModel::default(),
        )
        .await?;
        if self
            .attempts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            == 0
        {
            return Err(Refund::Refused);
        }
        Ok(())
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url().into(),
            ..Default::default()
        }),
        SeaOrmDatabaseModule,
        EventsModule,
        ResilienceModule::for_root(ResilienceConfig {
            retries: Some(1),
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            breaker_window: None,
            ..ResilienceConfig::default()
        }),
    ],
    providers = [ResilientListeners],
)]
struct ResilientModule;

/// A decorated listener's unit is its transaction whatever its policy
/// answers: a run failing or given up after its `Repo` write leaves nothing
/// committed, an answer commits its row, and an idempotent run commits its
/// retry's row alone, the failed attempt's rolled back in its savepoint.
#[tokio::test]
async fn a_decorated_listener_refused_or_failed_after_its_repo_write_leaves_nothing_committed() {
    let conn = crate::harness::connect().await;
    let table = "listener_unit_resilient";
    crate::harness::fresh_table(&conn, table).await;
    let logs = nest_rs_testing::LogCapture::install();
    let app = App::builder()
        .module::<ResilientModule>()
        .build()
        .await
        .expect("boots");

    for then in [Then::Fails, Then::Stalls] {
        emitted_off_every_unit(bus(&app), RefundAsked { then }).await;
        assert_eq!(
            crate::harness::committed_rows(&conn, table).await,
            0,
            "{then:?}: the run's error rolled its unit's write back",
        );
    }
    let failed = logs.find(
        nest_rs_events::TARGET,
        "event listener failed — dispatch continues with the next listener",
    );
    let errors: Vec<_> = failed.iter().map(|line| line.field("error")).collect();
    assert_eq!(errors.len(), 2, "{failed:#?}");
    assert!(
        errors[0]
            .as_deref()
            .is_some_and(|error| error.contains("the payment provider refused the refund")),
        "{failed:#?}"
    );
    assert!(
        errors[1]
            .as_deref()
            .is_some_and(|error| error.contains("the call did not answer within its timeout")),
        "the refusal, after the write: {failed:#?}"
    );

    emitted_off_every_unit(
        bus(&app),
        RefundAsked {
            then: Then::Answers,
        },
    )
    .await;
    assert_eq!(
        crate::harness::committed_rows(&conn, table).await,
        1,
        "an answer committed its unit's write",
    );

    emitted_off_every_unit(bus(&app), RestockAsked).await;
    assert_eq!(
        crate::harness::committed_rows(&conn, table).await,
        2,
        "the retry's row alone: the failed attempt's rolled back with its savepoint",
    );
}
