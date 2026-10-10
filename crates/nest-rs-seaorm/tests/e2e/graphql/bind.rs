//! `src/graphql/bind.rs`: a by-id load the database refuses logs the `DbErr`
//! and answers a bare `internal error`, GraphQL having no rendering choke point.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read, WireModelDefaults};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlOperationGuard, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard, async_trait};
use nest_rs_seaorm::{CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs_testing::{LogCapture, TestApp};
use serde::{Deserialize, Serialize};

/// An entity whose table is never created, so every read is a driver error.
mod ghost {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "bind_probe_table_that_does_not_exist")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub title: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

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
    /// Calls `bind`, which owns the `Found`/`Denied`/`Missing` answer and the
    /// error branch under test.
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
    // Thread-local suffices: `#[tokio::test]` runs every resolver on this thread.
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

    // A `DbErr` `Display` would name the table and, on a constraint, its values.
    let body_json: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
    assert_eq!(
        body_json["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "the client gets a generic error: {body}",
    );
    assert_eq!(
        body_json["errors"][0]["extensions"]["code"],
        nest_rs_core::problem::code::INTERNAL.as_str(),
        "under the one code every opaque failure carries: {body}",
    );
    assert!(
        !body.contains("bind_probe_table_that_does_not_exist"),
        "and nothing about the schema behind it: {body}",
    );

    let event = logs.expect_one(nest_rs_seaorm::TARGET, "by-id access load failed");
    assert_eq!(event.level, "error");
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
