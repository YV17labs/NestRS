//! `src/graphql/bind.rs`: what a by-id load says when the database refuses it.
//!
//! HTTP has a choke point — every opaque `ServiceError` logs once as it renders
//! — and GraphQL has none, so `bind` logs the real `DbErr` itself and hands the
//! client a bare `internal error`. That asymmetry is the whole reason the event
//! exists, and it means the log line is the *only* copy of the cause: schema,
//! column and constraint names never reach the wire, deliberately, so an
//! operator with no line here has nothing at all.
//!
//! Against live Postgres rather than in-process because the failure under test
//! is a driver error on a real statement — a hand-made `DbErr` would prove the
//! branch and not that the branch is reachable.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlOperationGuard, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard, async_trait};
use nest_rs_resource::WireModelDefaults;
use nest_rs_seaorm::{CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::{LogCapture, TestApp};
use serde::{Deserialize, Serialize};

/// An entity whose table is never created. Every read against it is a driver
/// error, which is the point: the failure comes from Postgres rather than from
/// a stub, so what the test exercises is the path a real outage takes.
mod ghost {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "bind_probe_table_that_does_not_exist")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub title: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl WireModelDefaults for ghost::Entity {
    fn fill_wire_defaults(_map: &mut serde_json::Map<String, serde_json::Value>) {}

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "title"])
    }
}

#[derive(SimpleObject, Serialize, Deserialize)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct Ghost {
    id: String,
    title: String,
}

impl From<ghost::Model> for Ghost {
    fn from(model: ghost::Model) -> Self {
        Self {
            id: model.id.to_string(),
            title: model.title,
        }
    }
}

#[injectable]
#[derive(Default)]
struct GhostsService;

impl CrudService for GhostsService {
    type Entity = ghost::Entity;
}

#[injectable]
#[derive(Default)]
struct ReadEverything;

impl AbilityFactory for ReadEverything {
    type Actor = ();

    fn define(&self, _actor: &(), ab: &mut AbilityBuilder) {
        ab.can(Action::Read, ghost::Entity);
    }
}

type GhostAbilityGuard = AbilityGuard<ReadEverything>;

#[injectable]
#[derive(Default)]
struct AlwaysAuthenticated;

impl Layer for AlwaysAuthenticated {}

#[async_trait]
impl Guard for AlwaysAuthenticated {
    async fn check_http(&self, req: &mut poem::Request) -> Result<(), Denial> {
        req.extensions_mut().insert(());
        Ok(())
    }
}

impl HttpGuard for AlwaysAuthenticated {}

type Bridge = GraphqlAbilityBridge<AlwaysAuthenticated, GhostAbilityGuard>;

#[resolver]
struct GhostsResolver;

#[operations]
impl GhostsResolver {
    /// Calls `bind` rather than a service method: it is `bind` that owns the
    /// three-way `Found`/`Denied`/`Missing` answer *and* the error branch under
    /// test, and a resolver written any other way would not reach it.
    #[query]
    #[authorize(Read, ghost::Entity)]
    async fn ghost(&self, ctx: &Context<'_>, id: String) -> GqlResult<Option<Ghost>> {
        Ok(
            nest_rs_seaorm::graphql::bind::<Read, GhostsService>(ctx, &id)
                .await?
                .map(Ghost::from),
        )
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
            }),
            SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig::default()),
    ],
    providers = [
        GhostsService,
        ReadEverything,
        GhostAbilityGuard,
        AlwaysAuthenticated,
        Bridge as dyn GraphqlOperationGuard,
        GhostsResolver,
    ],
)]
struct GhostModule;

const QUERY: &str = r#"query($id: String!) { ghost(id: $id) { id title } }"#;

#[tokio::test]
async fn a_failed_by_id_load_logs_the_driver_error_and_answers_generically() {
    // Thread-local: `#[tokio::test]` is a current-thread runtime, so the
    // endpoint's task and every resolver it drives run on this thread. The
    // global capture is permanent and one per process — spending it where the
    // ordinary one works is a cost with nothing bought.
    let logs = LogCapture::install();

    let app = TestApp::for_module::<GhostModule>()
        .await
        .expect("the subgraph boots — the table's absence is a runtime fact");

    let id = sea_orm::prelude::Uuid::now_v7().to_string();
    let body = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": QUERY,
            "variables": { "id": id },
        }))
        .send()
        .await
        .0
        .into_body()
        .into_string()
        .await
        .expect("a GraphQL response body");

    // What the client is told, and what it is not. A `DbErr` `Display` here
    // would name the table and, on a constraint violation, the values in it.
    assert!(
        body.contains("internal error") && body.contains("INTERNAL_SERVER_ERROR"),
        "the client gets a generic error with a programmable code: {body}",
    );
    assert!(
        !body.contains("bind_probe_table_that_does_not_exist"),
        "and nothing about the schema behind it: {body}",
    );

    let event = logs.expect_one(nest_rs_seaorm::TARGET, "by-id access load failed");
    assert_eq!(event.level, "error");
    // The service, because the resolver's name is not in the error and one
    // schema binds many: without it an operator has a driver error and no
    // subject.
    assert!(
        event.field("service").is_some_and(|s| s.contains("Ghosts")),
        "the event names the bound service, got {:?}",
        event.fields,
    );
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("bind_probe_table_that_does_not_exist")),
        "…and carries the driver's own message, which is the half the client \
         never sees, got {:?}",
        event.fields,
    );
}
