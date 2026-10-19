//! Covers `src/graphql/bind.rs`: the service `bind = Service` loads through is
//! inside the access contract the boot checks, as `Bind<A, S>`'s is, and a
//! wiring defect a hand-written `bind` meets answers opaquely, the type on the
//! operator's line.

use nest_rs_authz::{Read, Update};
use nest_rs_core::{AccessGraphError, injectable, module};
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_seaorm::{Authorized, CrudService};
use nest_rs_testing::{LogCapture, TestApp};

mod note {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
    #[sea_orm(table_name = "notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl nest_rs_authz::WireModelDefaults for note::Entity {}

#[injectable]
#[derive(Default)]
struct NotesService;

impl CrudService for NotesService {
    type Entity = note::Entity;
}

#[module(providers = [NotesService])]
struct NotesModule;

#[resolver]
struct NotesResolver;

#[operations]
impl NotesResolver {
    #[mutation]
    #[authorize(Update, bind = NotesService)]
    async fn touch_note(&self, note: Authorized<Update, note::Entity>) -> bool {
        let _ = note;
        true
    }
}

#[module(imports = [GraphqlModule::for_root(None)], providers = [NotesResolver])]
struct NotesGraphqlModule;

/// The service's module is reached through a sibling, never imported by the
/// resolver's own.
#[module(imports = [NotesModule, NotesGraphqlModule])]
struct StrayNotesRoot;

#[tokio::test]
async fn a_bound_service_whose_module_is_not_imported_fails_the_boot() {
    let refused = TestApp::for_module::<StrayNotesRoot>()
        .await
        .err()
        .expect("a `bind` over an unimported service must fail the boot");
    let access = refused
        .downcast::<AccessGraphError>()
        .unwrap_or_else(|refused| panic!("not the access violation: {refused:#}"));
    assert_eq!(access.consumer, "NotesResolver");
    assert_eq!(access.owner, "NotesModule");
}

/// Listed in no module: only a hand-written `bind` can reach it.
#[injectable]
#[derive(Default)]
struct UnlistedService;

impl CrudService for UnlistedService {
    type Entity = note::Entity;
}

#[injectable]
#[derive(Default)]
struct ListedService;

impl CrudService for ListedService {
    type Entity = note::Entity;
}

#[resolver]
struct HandBoundResolver;

#[operations]
impl HandBoundResolver {
    #[query]
    #[public]
    async fn unlisted_note(&self, ctx: &Context<'_>, id: String) -> GqlResult<bool> {
        let note = nest_rs_seaorm::graphql::bind::<Read, UnlistedService>(ctx, &id).await?;
        Ok(note.is_some())
    }

    #[query]
    #[public]
    async fn listed_note(&self, ctx: &Context<'_>, id: String) -> GqlResult<bool> {
        let note = nest_rs_seaorm::graphql::bind::<Read, ListedService>(ctx, &id).await?;
        Ok(note.is_some())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [ListedService, HandBoundResolver],
)]
struct HandBoundModule;

async fn answer(app: &TestApp, field: &str) -> serde_json::Value {
    let query = format!(r#"{{ {field}(id: "{}") }}"#, uuid::Uuid::now_v7());
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.json().await.value().deserialize()
}

#[tokio::test]
async fn a_bound_service_no_module_provides_answers_opaquely_and_files_its_type() {
    let app = TestApp::for_module::<HandBoundModule>()
        .await
        .expect("a hand-written `bind` is no declaration the boot reads");
    let logs = LogCapture::install();

    let body = answer(&app, "unlistedNote").await;

    assert_eq!(
        body["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{body}"
    );
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "INTERNAL",
        "{body}"
    );
    assert!(!body.to_string().contains("UnlistedService"), "{body}");
    let line = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "no provider for the bound service — add it to a module's providers",
    );
    assert_eq!(line.level, "error");
    assert!(
        line.field("provider")
            .is_some_and(|provider| provider.contains("UnlistedService")),
        "{:?}",
        line.fields,
    );
}

#[tokio::test]
async fn a_bind_with_no_ability_answers_opaquely_and_files_the_remedy() {
    let app = TestApp::for_module::<HandBoundModule>()
        .await
        .expect("boots");
    let logs = LogCapture::install();

    let body = answer(&app, "listedNote").await;

    assert_eq!(
        body["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{body}"
    );
    assert_eq!(
        body["errors"][0]["extensions"]["code"], "INTERNAL",
        "{body}"
    );
    assert!(!body.to_string().contains("Ability"), "{body}");
    let line = logs.expect_one(nest_rs_authz::TARGET, "authorization denied");
    assert_eq!(line.level, "error");
    assert!(line.field("remedy").is_some(), "{:?}", line.fields);
}
