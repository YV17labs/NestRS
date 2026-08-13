//! An `#[entity]` resolved by reference is scoped and masked like any other
//! operation, against live Postgres.
//!
//! `_entities` is the one field a client never writes: the router hands it
//! `{__typename, <key fields>}` for objects nobody named, and nothing in the
//! document mentions the resolver behind it. So what has to be true is that the
//! posture on it is real — that the row it hands back went through `Repo` under
//! the caller's ability, and that a row the ability does not reach is not
//! reachable *by key* either. Asserted here rather than in-process because the
//! filter that matters is the `WHERE` the ability adds.

use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::http::AbilityGuard;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult, SimpleObject};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlOperationGuard, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard, async_trait, guard};
use nest_rs_resource::WireModelDefaults;
use nest_rs_seaorm::{Access, CrudService, DatabaseConfig, DatabaseModule, ServiceError};
use nest_rs_testing::TestApp;
use sea_orm::prelude::Uuid;
use serde::{Deserialize, Serialize};

mod post {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize)]
    #[sea_orm(table_name = "federated_probe_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub title: String,
        pub published: bool,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Stands in for what `#[expose]` emits: `secret` carries no exposure, so the
/// mask strains it out of every value regardless of the grant.
impl WireModelDefaults for post::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry(String::from("secret"))
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "title", "published"])
    }
}

/// The federated type. Its `@key` is the entity resolver's `id` argument.
///
/// `crate = ` because this derive is the test's own, not one a decorator
/// emitted: async-graphql resolves its paths from the call site, and this crate
/// reaches it through `nest-rs-graphql`.
/// `published` is `Option` because the field grant below masks it: GraphQL
/// cannot ship a masked-out non-nullable field, so a maskable column is
/// nullable on the wire or the whole operation fails closed.
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

/// Grants exactly the published rows — so "the ability filtered this" and "the
/// row does not exist" are two different states the assertions can tell apart.
///
/// The grant is also narrowed to two **columns**, and that half is what gives
/// the mask something to strain: `secret` is stripped by the static expose set
/// whether or not masking runs, so a test asserting only its absence passes
/// against an entity resolver that never masks at all. `published` is exposed
/// *and* withheld by the grant, so it can only be absent if the mask ran.
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

/// Authenticates every caller as the same principal. What is under test is the
/// **ability**, not who the caller is — and on GraphQL a visitor ability
/// deliberately cannot satisfy an `#[authorize]`, so a principal is what lets
/// this file assert on the rule rather than on the absence of one.
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
            // A row the ability does not reach is `Denied`; one that is not
            // there is `Missing`. Neither is resolvable by key, and the router
            // is told the same thing for both — an entity resolver that
            // distinguished them would be an existence oracle keyed by id.
            Ok(Access::Denied | Access::Missing) => Ok(None),
            Err(err) => Err(nest_rs_graphql::async_graphql::Error::new(
                ServiceError::from(err).to_string(),
            )),
        }
    }
}

#[module(
    imports = [
        DatabaseModule::for_root(DatabaseConfig {
            url: crate::harness::url(),
            ..Default::default()
        }),
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
