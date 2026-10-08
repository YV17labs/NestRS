//! An `#[entity]` resolved by reference is scoped and masked like any other
//! operation: a row the ability does not reach is not reachable by key either.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlOperationGuard, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard, async_trait, guard};
use nest_rs_resource::WireModelDefaults;
use nest_rs_seaorm::{
    Access, CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, ServiceError,
};
use nest_rs_testing::TestApp;
use sea_orm::prelude::Uuid;
use serde::{Deserialize, Serialize};

mod post {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "federated_probe_posts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub title: String,
        pub published: bool,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Stands in for what `#[expose]` emits: `secret` carries no exposure.
impl WireModelDefaults for post::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry(String::from("secret"))
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "title", "published"])
    }
}

/// The federated type; its `@key` is the entity resolver's `id` argument.
///
/// `crate = `: async-graphql resolves its paths from the call site. `published`
/// is `Option`: GraphQL cannot ship a masked-out non-nullable field.
#[derive(SimpleObject, Serialize, Deserialize)]
#[graphql(crate = "::nest_rs_graphql::async_graphql")]
struct Post {
    id: String,
    title: String,
    published: Option<bool>,
}

impl From<post::Model> for Post {
    fn from(model: post::Model) -> Self {
        Self {
            id: model.id.to_string(),
            title: model.title,
            published: Some(model.published),
        }
    }
}

struct PostsService;

impl CrudService for PostsService {
    type Entity = post::Entity;
}

/// Grants the published rows and two columns: `published` is exposed yet
/// withheld, so only the mask can strip it (the expose set alone strips `secret`).
#[injectable]
#[derive(Default)]
struct PublishedOnly;

impl AbilityFactory for PublishedOnly {
    type Actor = ();

    fn define(&self, _actor: &(), ab: &mut AbilityBuilder) {
        ab.can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::Published, true))
            .fields([post::Column::Id, post::Column::Title]);
    }
}

type PostsAbilityGuard = AbilityGuard<PublishedOnly>;

/// Authenticates every caller: on GraphQL a visitor ability cannot satisfy an
/// `#[authorize]`.
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

/// The subgraph's operation guard, aliased because a `providers = [...]` entry
/// is one type path — a generic's comma would read as the next provider.
type Bridge = GraphqlAbilityBridge<AlwaysAuthenticated, PostsAbilityGuard>;

#[resolver]
struct PostsResolver;

#[operations]
impl PostsResolver {
    /// Resolved by reference. `#[authorize]` is what gates it and masks what it
    /// returns; `access` is what puts the ability's `WHERE` on the read.
    #[entity]
    #[authorize(Read, post::Entity)]
    async fn find_post_by_id(&self, _ctx: &Context<'_>, id: String) -> GqlResult<Option<Post>> {
        let Ok(key) = id.parse::<Uuid>() else {
            return Ok(None);
        };
        match PostsService.access(Action::Read, key).await {
            Ok(Access::Found(model)) => Ok(Some(Post::from(model))),
            // Answered alike: telling them apart is an existence oracle by id.
            Ok(Access::Denied | Access::Missing) => Ok(None),
            Err(err) => Err(nest_rs_graphql::async_graphql::Error::new(
                ServiceError::from(err).to_string(),
            )),
        }
    }
}

#[module(
    imports = [
        SeaOrmModule::for_root(SeaOrmConfig {
            url: crate::harness::url(),
            ..Default::default()
            }),
            SeaOrmDatabaseModule,
        GraphqlModule::for_root(GraphqlConfig {
            federation: true,
            ..GraphqlConfig::default()
        }),
    ],
    providers = [
        PublishedOnly,
        PostsAbilityGuard,
        AlwaysAuthenticated,
        Bridge as dyn GraphqlOperationGuard,
        PostsResolver,
    ],
)]
struct FederatedModule;

const REFERENCE: &str = r#"
    query($reps: [_Any!]!) {
      _entities(representations: $reps) { ... on Post { id title published } }
    }
"#;

async fn boot() -> TestApp {
    crate::harness::setup_shared_table(
        &crate::harness::connect().await,
        "federated_probe_posts",
        "CREATE TABLE IF NOT EXISTS federated_probe_posts (
            id UUID PRIMARY KEY,
            title TEXT NOT NULL,
            published BOOLEAN NOT NULL,
            secret TEXT NOT NULL
        );
         INSERT INTO federated_probe_posts (id, title, published, secret) VALUES
            ('11111111-1111-4111-8111-111111111111', 'published', true,  'sauce-1'),
            ('22222222-2222-4222-8222-222222222222', 'a draft',   false, 'sauce-2')
         ON CONFLICT (id) DO NOTHING;",
    )
    .await;
    TestApp::builder()
        .module::<FederatedModule>()
        .use_guards_global([guard::<AlwaysAuthenticated>(), guard::<PostsAbilityGuard>()])
        .build()
        .await
        .expect("the subgraph boots against live Postgres")
}

/// The published row, and the draft the visitor ability withholds.
const PUBLISHED: &str = "11111111-1111-4111-8111-111111111111";
const DRAFT: &str = "22222222-2222-4222-8222-222222222222";

async fn resolve(app: &TestApp, id: &str) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": REFERENCE,
            "variables": { "reps": [{ "__typename": "Post", "id": id }] },
        }))
        .send()
        .await;
    resp.json().await.value().deserialize::<serde_json::Value>()
}

#[tokio::test]
async fn a_row_the_ability_reaches_resolves_by_key_and_arrives_masked() {
    let app = boot().await;
    let body = resolve(&app, PUBLISHED).await;
    let entity = &body["data"]["_entities"][0];

    assert_eq!(
        entity["title"], "published",
        "the reference reached the row through `Repo` under the caller's ability: {body}",
    );
    assert!(
        entity["published"].is_null(),
        "and the mask strained the column the field grant withholds, exactly as \
         it does on a `#[query]` — with no masking call anywhere in the entity \
         resolver's body, `#[authorize]` being the whole declaration: {body}",
    );
    assert!(
        entity.get("secret").is_none(),
        "the unexposed column never reaches the wire either — though the static \
         expose set alone would do that, which is why it is not the witness: \
         {body}",
    );
}

#[tokio::test]
async fn a_row_the_ability_does_not_reach_is_not_reachable_by_key_either() {
    let app = boot().await;
    let body = resolve(&app, DRAFT).await;

    assert!(
        body["data"]["_entities"][0].is_null(),
        "the draft is a real row, and the ability's `WHERE` is what withholds \
         it — otherwise `_entities` is a way to read past every rule in the \
         schema by naming a key: {body}",
    );
    assert!(
        !body.to_string().contains("sauce-2"),
        "and nothing of it leaks through the error either: {body}",
    );
}
