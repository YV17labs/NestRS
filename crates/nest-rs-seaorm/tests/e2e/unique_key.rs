//! `src/unique_key.rs` against Postgres: the boot's read of the catalog on a pool
//! as `SeaOrmModule` opens it. It waits for a connection another factory is still
//! handing back, so a conflict names its field; it reads through a partition, the
//! table a violation reports; and a server that stopped answering fails the boot
//! at the read's net, the one wait the pool and Postgres do not bound themselves.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nest_rs_core::{Collecting, ContainerBuilder, Module, Registering, module};
use nest_rs_http::{Method, StatusCode};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::{EphemeralDatabase, TestApp};
use sea_orm::sqlx::postgres::PgConnectOptions;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::harness::{ProbeMigrator, ProbeTables};

mod handle {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Handle")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "unique_key_probe_handles")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create))]
        pub email: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct HandlesService;

impl nest_rs_seaorm::CrudService for HandlesService {
    type Entity = handle::Entity;
}

impl nest_rs_seaorm::Creatable for HandlesService {
    type Create = handle::CreateHandle;
}

#[nest_rs_core::injectable]
#[derive(Default)]
struct EveryHandle;

impl nest_rs_authz::AbilityFactory for EveryHandle {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut nest_rs_authz::AbilityBuilder) {
        ab.can(nest_rs_authz::Action::Manage, handle::Entity);
    }
}

#[nest_rs_http::controller(path = "/handles")]
#[use_guards(
    crate::always_authenticated::AlwaysAuthenticated,
    nest_rs_authz::AbilityGuard<EveryHandle>
)]
struct HandlesController {
    #[inject]
    svc: Arc<HandlesService>,
}

#[nest_rs_http::crud(service = HandlesService, ops = [create])]
impl HandlesController {}

#[module(
    imports = [SeaOrmModule, SeaOrmDatabaseModule],
    providers = [
        HandlesService,
        EveryHandle,
        nest_rs_authz::AbilityGuard<EveryHandle>,
        crate::always_authenticated::AlwaysAuthenticated,
        HandlesController,
    ],
)]
struct HandlesModule;

/// What an address's second create answered: its status and its `errors`.
async fn second_create(app: &TestApp, email: &str) -> (StatusCode, Option<Value>) {
    let body = json!({ "email": email });
    let send = || {
        app.http()
            .request(Method::POST, "/handles")
            .body_json(&body)
            .send()
    };
    send().await.assert_status(StatusCode::CREATED);
    let resp = send().await;
    let status = resp.0.status();
    let problem = resp.json().await.value().deserialize::<Value>();
    (status, problem.get("errors").cloned())
}

/// A provider whose factory reads the pool before the binding's: a lookup
/// table, a check at boot. sqlx hands its connection back on a task of its own,
/// after a ping, so the binding's read comes with none idle.
struct Touched;

struct TouchModule;

impl Module for TouchModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }

    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_factory_after::<Touched, (DatabaseConnection,), _, _>(
            |container| async move {
                let db = container
                    .get::<DatabaseConnection>()
                    .ok_or_else(|| anyhow::anyhow!("no pool"))?;
                db.execute_unprepared("SELECT 1").await?;
                Ok(Touched)
            },
        )
    }
}

#[tokio::test]
async fn a_connection_on_its_way_back_is_waited_for_and_the_conflict_names_its_field() {
    let db = EphemeralDatabase::create::<crate::harness::NoMigrations>()
        .await
        .expect("a database of the test's own");
    db.connection()
        .execute_unprepared(
            "CREATE TABLE unique_key_probe_handles (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL CONSTRAINT unique_key_probe_handles_email_key UNIQUE
            )",
        )
        .await
        .expect("the probe table");
    for boot in 0..5 {
        let pool = Database::connect(crate::harness::options(db.url().to_owned()))
            .await
            .expect("a pool of its own, proved, as the boot opens it");
        let app = TestApp::builder()
            .provide_arc(Arc::new(pool))
            .module::<TouchModule>()
            .module::<HandlesModule>()
            .build()
            .await
            .expect("boots");

        let (status, errors) = second_create(&app, &format!("boot-{boot}@probe.test")).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            errors,
            Some(json!({ "email": [{ "code": "unique" }] })),
            "boot {boot}",
        );
    }
}

struct PartitionedTables;

impl ProbeTables for PartitionedTables {
    fn ddl() -> String {
        "CREATE TABLE unique_key_probe_handles (
            id UUID NOT NULL,
            email TEXT NOT NULL,
            PRIMARY KEY (id, email),
            UNIQUE (email)
        ) PARTITION BY HASH (email);
        CREATE TABLE unique_key_probe_handles_0 PARTITION OF unique_key_probe_handles
            FOR VALUES WITH (MODULUS 2, REMAINDER 0);
        CREATE TABLE unique_key_probe_handles_1 PARTITION OF unique_key_probe_handles
            FOR VALUES WITH (MODULUS 2, REMAINDER 1);"
            .to_owned()
    }
}

/// Postgres reports the partition and the partition's own index, never the
/// table the entity names, which the read follows through `pg_inherits`.
#[tokio::test]
async fn a_key_of_a_partitioned_table_names_its_field() {
    let app = TestApp::builder()
        .module::<HandlesModule>()
        .ephemeral_database::<ProbeMigrator<PartitionedTables>>()
        .build()
        .await
        .expect("boots");

    for email in ["ada@probe.test", "grace@probe.test", "hopper@probe.test"] {
        let (status, errors) = second_create(&app, email).await;
        assert_eq!(status, StatusCode::CONFLICT, "{email}");
        assert_eq!(
            errors,
            Some(json!({ "email": [{ "code": "unique" }] })),
            "{email}",
        );
    }
}

/// A TCP relay to the suite's Postgres, on a name its certificate carries, that
/// swallows both directions once `stalled` is set: a server still connected
/// that stopped answering. The suite's URL rewritten to reach it.
async fn relay_to_postgres(stalled: Arc<AtomicBool>) -> String {
    let url = crate::harness::url();
    let server: PgConnectOptions = url.parse().expect("the suite's Postgres URL");
    let server = format!("{}:{}", server.get_host(), server.get_port());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let port = listener.local_addr().expect("its port").port();
    tokio::spawn(async move {
        loop {
            let (client, _) = listener.accept().await.expect("the pool dials the relay");
            let server = TcpStream::connect(&server).await.expect("Postgres answers");
            tokio::spawn(relaying(client, server, Arc::clone(&stalled)));
        }
    });
    nest_rs_testing::url_at(&url, format!("localhost:{port}"))
}

async fn relaying(mut client: TcpStream, mut server: TcpStream, stalled: Arc<AtomicBool>) {
    let (mut client_read, mut client_write) = client.split();
    let (mut server_read, mut server_write) = server.split();
    let (mut sent, mut answered) = ([0_u8; 8192], [0_u8; 8192]);
    loop {
        tokio::select! {
            read = client_read.read(&mut sent) => {
                let read = read.unwrap_or(0);
                if read == 0 {
                    return;
                }
                if !stalled.load(Ordering::SeqCst) {
                    server_write.write_all(&sent[..read]).await.ok();
                }
            }
            read = server_read.read(&mut answered) => {
                let read = read.unwrap_or(0);
                if read == 0 {
                    return;
                }
                if !stalled.load(Ordering::SeqCst) {
                    client_write.write_all(&answered[..read]).await.ok();
                }
            }
        }
    }
}

/// The net is the pool's acquire budget and the settle net above: the read waits
/// for neither the pool, whose test before acquire is off, nor Postgres, whose
/// bound never reaches a client it stopped answering. Past it the boot fails
/// naming the read.
#[tokio::test]
async fn a_server_that_stopped_answering_fails_the_boot_at_the_reads_net() {
    let stalled = Arc::new(AtomicBool::new(false));
    let mut options = crate::harness::options(relay_to_postgres(Arc::clone(&stalled)).await);
    options.max_connections(1).test_before_acquire(false);
    let pool = Database::connect(options)
        .await
        .expect("the pool is proved through the relay");
    stalled.store(true, Ordering::SeqCst);

    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let refused = TestApp::builder()
        .provide_arc(Arc::new(pool))
        .provide(SeaOrmConfig::default())
        .module::<HandlesModule>()
        .build()
        .await
        .err()
        .expect("nothing answers the read");

    let waited = started.elapsed();
    assert!(
        (Duration::from_secs(26)..Duration::from_secs(27)).contains(&waited),
        "10 s + 15 s + 1 s, on a clock nothing moves but the timers: {waited:?}",
    );
    let said = format!("{refused:#}");
    assert!(
        said.contains("SeaOrmDatabaseModule: could not read the unique keys"),
        "{said}",
    );
    assert!(said.contains("no answer within 26s"), "{said}");
}
