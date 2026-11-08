//! An `#[entity]` resolved by reference is scoped and judged like any other
//! operation: a row the ability does not reach is not reachable by key either.

use nest_rs_authz::AbilityGuard;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action, Read};
use nest_rs_core::{injectable, module};
use nest_rs_graphql::GraphqlView;
use nest_rs_graphql::async_graphql::{Context, Result as GqlResult};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_guards::GlobalGuards;
use nest_rs_http::EndpointGuards;
use nest_rs_seaorm::{
    Access, CrudService, SeaOrmConfig, SeaOrmDatabaseModule, SeaOrmModule, ServiceError,
};
use nest_rs_testing::TestApp;
use sea_orm::prelude::Uuid;

use crate::always_authenticated::AlwaysAuthenticated;

mod post {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    /// The federated type; its `@key` is the entity resolver's `id` argument.
    #[expose(name = "Post", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "federated_probe_posts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: String,
        #[expose(nullable)]
        pub published: bool,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

struct PostsService;

impl CrudService for PostsService {
    type Entity = post::Entity;
}

/// Grants the published rows and two columns: `published` is exposed yet
/// withheld, so only the row's judgement can leave it out (the expose set alone
/// leaves out `secret`).
#[injectable]
#[derive(Default)]
struct PublishedOnly;

impl AbilityFactory for PublishedOnly {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::Published, true))
            .fields([post::Column::Id, post::Column::Title]);
    }
}

type PostsAbilityGuard = AbilityGuard<PublishedOnly>;

#[resolver]
struct PostsResolver;

#[operations]
impl PostsResolver {
    /// Resolved by reference. `#[authorize]` is what gates it and opens its read;
    /// `access` is what puts the ability's `WHERE` on the read.
    #[entity]
    #[authorize(Read, post::Entity)]
    async fn find_post_by_id(
        &self,
        _ctx: &Context<'_>,
        id: String,
    ) -> GqlResult<Option<post::Post>> {
        let Ok(key) = id.parse::<Uuid>() else {
            return Ok(None);
        };
        match PostsService.access(Action::Read, key).await {
            Ok(Access::Found(model)) => Ok(Some(post::Post::from(&model))),
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
            url: crate::harness::url().into(),
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
        EndpointGuards<GraphqlView, (AlwaysAuthenticated, PostsAbilityGuard)>,
        PostsResolver,
    ],
)]
struct FederatedModule;

const REFERENCE: &str = r#"
    query($reps: [_Any!]!) {
      _entities(representations: $reps) { ... on Post { id title published } }
    }
"#;

#[module(
    imports = [FederatedModule],
    providers = [GlobalGuards<(AlwaysAuthenticated, PostsAbilityGuard)>],
)]
struct BootApp;

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
        .module::<BootApp>()
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
async fn a_row_the_ability_reaches_resolves_by_key_and_arrives_judged() {
    let app = boot().await;
    let body = resolve(&app, PUBLISHED).await;
    let entity = &body["data"]["_entities"][0];

    assert_eq!(
        entity["title"], "published",
        "the reference reached the row through `Repo` under the caller's ability: {body}",
    );
    assert!(
        entity["published"].is_null(),
        "and the row's judgement withheld the column the field grant leaves out, \
         exactly as on a `#[query]` — with no call anywhere in the entity \
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
