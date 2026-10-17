//! A listener's unit of work over `SeaOrmUnitOfWork`, held for its emitter's
//! commit: its writes are one transaction settled on its answer, an event it
//! emits waits for that commit, a pool listener's writes stand on their own,
//! and a listener cut at its timeout rolls back and gives its connection back.
//!
//! Each test runs over a one-connection pool and counts through it, so a count
//! waits for the connection the listener's settle gives back.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{App, injectable, module};
use nest_rs_database::{ExecutorScope, TransactionPolicy, UnitOfWork, UnitSpec, run_unit};
use nest_rs_events::{EventBus, EventsModule, listeners};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmUnitOfWork, current_executor};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

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

/// A table recreated per test, so what a listener left is a row count.
async fn scratch_table(conn: &DatabaseConnection, name: &str) {
    for sql in [
        format!("DROP TABLE IF EXISTS {name}"),
        format!("CREATE TABLE {name} (id serial PRIMARY KEY)"),
    ] {
        conn.execute_unprepared(&sql)
            .await
            .expect("the scratch table is created");
    }
}

async fn row_count(conn: &impl ConnectionTrait, name: &str) -> i64 {
    conn.query_one_raw(Statement::from_string(
        conn.get_database_backend(),
        format!("SELECT count(*)::bigint AS n FROM {name}"),
    ))
    .await
    .expect("the count query runs")
    .expect("count returns a row")
    .try_get::<i64>("", "n")
    .expect("the count is an integer")
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
    scratch_table(&conn, table).await;
    let app = App::new::<WritingModule>().expect("boots");

    emitted_in_a_unit(&conn, &bus(&app), TwoRowsAsked { table, fails: true }).await;
    assert_eq!(
        row_count(&*conn, table).await,
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
        row_count(&*conn, table).await,
        2,
        "a listener answering Ok committed both writes"
    );
}

#[tokio::test]
async fn a_pool_listeners_first_write_stands_after_its_err() {
    let conn = one_connection().await;
    let table = "listener_unit_on_the_pool";
    scratch_table(&conn, table).await;
    let app = App::new::<WritingModule>().expect("boots");

    emitted_in_a_unit(&conn, &bus(&app), OnThePoolAsked { table }).await;

    assert_eq!(
        row_count(&*conn, table).await,
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
        let committed = row_count(&executor, event.table).await;
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
    scratch_table(&conn, table).await;
    let app = App::new::<ChainModule>().expect("boots");
    let probe = app
        .container()
        .get::<ChainProbe>()
        .expect("the probe is provided");

    emitted_in_a_unit(&conn, &bus(&app), Published { table, fails: true }).await;
    assert!(
        probe.seen().is_empty(),
        "the listener rolled back, so the fact it announced never happened",
    );
    assert_eq!(row_count(&*conn, table).await, 0);

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
    scratch_table(&conn, table).await;
    let app = App::new::<StallingModule>().expect("boots");

    let emitted = std::time::Instant::now();
    emitted_in_a_unit(&conn, &bus(&app), StallAsked { table }).await;
    let took = emitted.elapsed();

    assert!(
        took < Duration::from_secs(10),
        "cut at its 1 s timeout, not after its 30 s sleep: {took:?}",
    );
    assert_eq!(
        row_count(&*conn, table).await,
        0,
        "the cut listener's write rolled back, and the pool's one connection came back",
    );
}
