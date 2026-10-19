//! `src/graphql/bind.rs`: a by-id load the database refuses logs the `DbErr`
//! and answers a bare `internal error`, GraphQL having no rendering choke point.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read, WireModelDefaults};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::GraphqlView;
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs_http::EndpointGuards;
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
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
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
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(crate::Anyone);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AlwaysAuthenticated {}

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
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, GhostAbilityGuard)>,
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

    let event = logs.expect_one(nest_rs_seaorm::target::ORM, "by-id access load failed");
    assert_eq!(event.level, "error");
    assert!(
        event.field("service").is_some_and(|s| s.contains("Ghosts")),
        "the event names the bound service, got {:?}",
        event.fields,
    );
    assert!(
        event
            .field("error")
            .is_some_and(|e| e.contains("database error 42P01")),
        "…and carries the database's SQLSTATE, which is the half the client \
         never sees, got {:?}",
        event.fields,
    );
}

/// The ability a field's own guard attaches is the one `bind` loads under:
/// the unit's, which the class gate checked — never one off the GraphQL
/// context.
mod ability_of_the_unit {
    use nest_rs_authz::{AbilityBuilder, Action, Read};
    use nest_rs_core::{Layer, UnitContext, injectable, module};
    use nest_rs_graphql::async_graphql::{Context, Result as GqlResult};
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
    use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
    use nest_rs_seaorm::{SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule};
    use nest_rs_testing::{LogCapture, TestApp};

    use super::{Ghost, GhostsService, QUERY, ghost};

    /// Attaches, at the field, an ability reading every ghost.
    #[injectable]
    #[derive(Default)]
    struct FieldAbility;

    impl Layer for FieldAbility {}

    #[async_trait]
    impl Guard for FieldAbility {
        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let mut builder = AbilityBuilder::new();
            builder.can(Action::Read, ghost::Entity);
            cx.attach(builder.build().expect("a valid ability"));
            Ok(())
        }
    }

    impl GuardFor<GraphqlView> for FieldAbility {}

    #[resolver]
    struct FieldGhostsResolver;

    #[operations]
    impl FieldGhostsResolver {
        #[query]
        #[authorize(Read, ghost::Entity)]
        #[use_guards(FieldAbility)]
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
        providers = [GhostsService, FieldAbility, FieldGhostsResolver],
    )]
    struct FieldGhostModule;

    #[tokio::test]
    async fn bind_loads_under_the_ability_the_class_gate_checked() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<FieldGhostModule>()
            .await
            .expect("boots");

        let id = sea_orm::prelude::Uuid::now_v7().to_string();
        let body: serde_json::Value = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": QUERY, "variables": { "id": id } }))
            .send()
            .await
            .json()
            .await
            .value()
            .deserialize();

        assert_eq!(
            body["errors"][0]["message"],
            nest_rs_core::OPAQUE_CLIENT_MESSAGE,
            "the load ran, and the absent table failed it: {body}",
        );
        logs.expect_one(nest_rs_seaorm::target::ORM, "by-id access load failed");
    }
}
