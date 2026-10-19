//! Automatic response masking through the real macros: `#[authorize(Action,
//! Entity)]` on a `#[query]` makes `#[resolver]` emit `masked_value_for`
//! around the return value — the resolver body writes no masking call. Covers
//! the wire shapes the wrapper sees through (bare DTO, `Vec`), the fail-closed
//! path (a required field the mask strips ⇒ GraphQL error, never unmasked
//! data), and the `#[public]` opt-out.

use nest_rs_graphql::GraphqlView;
use nest_rs_http::EndpointGuards;
use std::sync::Arc;

use nest_rs_authz::{
    AbilityBuilder, Action, MaskReplyError, Read, WireModelDefaults, masked_reply, with_ability,
};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream, StreamExt};
use nest_rs_graphql::async_graphql::{
    Error as GqlError, Executor, Request as GqlRequest, Result as GqlResult, SimpleObject,
};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::async_trait;
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::query;

/// A throwaway SeaORM entity with a server-only column (`secret`) the wire
/// DTOs never carry — [`WireModelDefaults`] reconstructs it for policy and the
/// exposed-key strainer drops it again.
mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

impl WireModelDefaults for widget::Entity {
    fn fill_wire_defaults(map: &mut serde_json::Map<String, serde_json::Value>) {
        map.entry("secret")
            .or_insert(serde_json::Value::String(String::new()));
    }

    fn wire_keys() -> Option<&'static [&'static str]> {
        Some(&["id", "name"])
    }

    fn is_placeholdered(column: widget::Column) -> bool {
        matches!(column, widget::Column::Secret)
    }
}

/// The wire shape: `name` optional so a field-restricted mask yields `None`
/// rather than an irreconcilable value.
#[derive(Clone, Debug, SimpleObject, Serialize, Deserialize)]
struct WidgetDto {
    id: i32,
    name: Option<String>,
}

/// A wire shape with a **required** `name`: when the mask strips it, the
/// masked value can no longer be deserialized — the operation must fail
/// closed.
#[derive(SimpleObject, Serialize, Deserialize)]
struct StrictWidgetDto {
    id: i32,
    name: String,
}

/// No-op stand-in for the authentication guard ahead of the ability guard at
/// the request.
#[injectable]
#[derive(Default)]
struct PassGuard;

impl Layer for PassGuard {}

#[async_trait]
impl Guard for PassGuard {}

/// Builds the caller's ability from an `x-role` header: `admin` reads widgets
/// unrestricted; `viewer` reads widgets but only the `id` field; `auditor`
/// reads only widget 1 and only its `id`; anyone else gets nothing.
#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let role = nest_rs_http::HttpView::of(cx)?
            .request()
            .header("x-role")
            .unwrap_or_default()
            .to_string();
        let mut b = AbilityBuilder::new();
        match role.as_str() {
            "admin" => {
                b.can(Action::Read, widget::Entity);
            }
            "viewer" => {
                b.can(Action::Read, widget::Entity)
                    .fields([widget::Column::Id]);
            }
            "auditor" => {
                b.can(Action::Read, widget::Entity)
                    .when(|p| p.eq(widget::Column::Id, 1))
                    .fields([widget::Column::Id]);
            }
            _ => {}
        }
        cx.attach(b.build().expect("valid test ability"));
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AbilityInjector {}

impl GuardFor<nest_rs_http::HttpView> for PassGuard {}

#[resolver]
struct MaskResolver {
    #[inject]
    feed: Arc<WidgetFeed>,
}

#[operations]
impl MaskResolver {
    #[query]
    #[authorize(Read, widget::Entity)]
    async fn widget(&self) -> GqlResult<WidgetDto> {
        Ok(WidgetDto {
            id: 1,
            name: Some("ada".into()),
        })
    }

    #[query]
    #[authorize(Read, widget::Entity)]
    async fn widgets(&self) -> GqlResult<Vec<WidgetDto>> {
        Ok(vec![
            WidgetDto {
                id: 1,
                name: Some("ada".into()),
            },
            WidgetDto {
                id: 2,
                name: Some("grace".into()),
            },
        ])
    }

    /// The posture on a bare return: the gate's denial and the mask's failure
    /// reach the client through the wrapper's `Result`, so the developer's
    /// method need not be fallible to carry either.
    #[query]
    #[authorize(Read, widget::Entity)]
    async fn bare_widget(&self) -> WidgetDto {
        WidgetDto {
            id: 1,
            name: Some("ada".into()),
        }
    }

    #[query]
    #[authorize(Read, widget::Entity)]
    async fn strict_widget(&self) -> GqlResult<StrictWidgetDto> {
        Ok(StrictWidgetDto {
            id: 1,
            name: "ada".into(),
        })
    }

    #[query]
    #[authorize(Read, widget::Entity)]
    async fn strict_widgets(&self) -> GqlResult<Vec<StrictWidgetDto>> {
        Ok(vec![
            StrictWidgetDto {
                id: 1,
                name: "ada".into(),
            },
            StrictWidgetDto {
                id: 2,
                name: "grace".into(),
            },
        ])
    }

    #[query]
    #[public]
    async fn motd(&self) -> GqlResult<String> {
        Ok("hello".into())
    }

    /// The stream both subscribers read; the resolver writes no masking call.
    #[subscription]
    #[authorize(Read, widget::Entity)]
    async fn widget_feed(&self) -> Result<impl Stream<Item = WidgetDto>, GqlError> {
        let rx = self.feed.tx.subscribe();
        Ok(stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(widget) => Some((widget, rx)),
                Err(_) => None,
            }
        }))
    }
}

/// The one source both subscribers read from, so a difference in what they
/// receive can only come from the posture.
#[injectable]
struct WidgetFeed {
    tx: broadcast::Sender<WidgetDto>,
}

impl Default for WidgetFeed {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(8).0,
        }
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        PassGuard,
        AbilityInjector,
        EndpointGuards<GraphqlView, (PassGuard, AbilityInjector)>,
        WidgetFeed,
        MaskResolver,
    ],
)]
struct MaskGraphqlModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<MaskGraphqlModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

#[tokio::test]
async fn unrestricted_caller_sees_the_field() {
    let app = boot().await;
    let json = query(&app, "admin", "{ widget { id name } }").await;
    assert_eq!(json["data"]["widget"]["id"], 1);
    assert_eq!(json["data"]["widget"]["name"], "ada");
}

#[tokio::test]
async fn restricted_field_is_masked_to_null() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ widget { id name } }").await;
    assert_eq!(json["data"]["widget"]["id"], 1);
    assert_eq!(
        json["data"]["widget"]["name"],
        serde_json::Value::Null,
        "the resolver returned Some(\"ada\") — the emitted mask must strip it"
    );
}

#[tokio::test]
async fn every_row_of_a_vec_is_masked() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ widgets { id name } }").await;
    let rows = json["data"]["widgets"]
        .as_array()
        .expect("widgets is a list");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["name"], serde_json::Value::Null);
    }
}

// `#[expose]` types a non-nullable column as a non-null GraphQL field, which the
// mask cannot null: the selection set decides.

#[tokio::test]
async fn selecting_a_field_the_grant_strips_is_refused() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ strictWidget { id name } }").await;
    assert_eq!(
        json["data"],
        serde_json::Value::Null,
        "no partial unmasked data may ship: {json}",
    );
    assert_eq!(
        json["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "a field outside the grant is a denial, not a masking failure: {json}",
    );
    // A list, not a comma-joined string.
    assert_eq!(
        json["errors"][0]["extensions"]["fields"],
        serde_json::json!(["name"]),
        "the denial names the field it refused, as a list: {json}",
    );
}

#[tokio::test]
async fn selecting_only_granted_fields_still_serves_the_entity() {
    let app = boot().await;
    let json = query(&app, "viewer", "{ strictWidget { id } }").await;
    assert!(
        json.get("errors").is_none(),
        "a query asking only for granted columns must succeed: {json}",
    );
    assert_eq!(json["data"]["strictWidget"]["id"], 1);
}

#[tokio::test]
async fn an_unrestricted_caller_reads_every_field_of_a_strict_shape() {
    let app = boot().await;
    let json = query(&app, "admin", "{ strictWidget { id name } }").await;
    assert_eq!(json["data"]["strictWidget"]["name"], "ada", "{json}");
}

#[tokio::test]
async fn rows_the_ability_refuses_are_still_dropped_from_a_strict_list() {
    let app = boot().await;
    let json = query(&app, "auditor", "{ strictWidgets { id } }").await;
    let rows = json["data"]["strictWidgets"]
        .as_array()
        .unwrap_or_else(|| panic!("strictWidgets is a list: {json}"));
    assert_eq!(rows.len(), 1, "widget 2 is outside the grant: {json}");
    assert_eq!(rows[0]["id"], 1);
}

#[tokio::test]
async fn a_caller_with_no_grant_reads_no_strict_row() {
    let app = boot().await;
    let json = query(&app, "", "{ strictWidgets { id } }").await;
    assert_eq!(
        json["data"],
        serde_json::Value::Null,
        "the class gate rejects a caller with no Read rule: {json}",
    );
    assert!(!json["errors"].as_array().unwrap_or(&vec![]).is_empty());
}

#[tokio::test]
async fn a_bare_return_operation_is_gated_and_masked() {
    let app = boot().await;
    let admin = query(&app, "admin", "{ bareWidget { id name } }").await;
    assert_eq!(admin["data"]["bareWidget"]["name"], "ada", "{admin}");
    let viewer = query(&app, "viewer", "{ bareWidget { id name } }").await;
    assert_eq!(viewer["data"]["bareWidget"]["id"], 1, "{viewer}");
    assert_eq!(
        viewer["data"]["bareWidget"]["name"],
        serde_json::Value::Null,
        "the mask strips a bare return as it does a fallible one: {viewer}",
    );
    let nobody = query(&app, "", "{ bareWidget { id name } }").await;
    assert!(
        !nobody["errors"].as_array().unwrap_or(&vec![]).is_empty()
            && !nobody.to_string().contains("ada"),
        "the class gate refuses a bare return as it does a fallible one: {nobody}",
    );
}

#[tokio::test]
async fn zero_grant_caller_is_gated_before_masking() {
    let app = boot().await;
    let json = query(&app, "", "{ widget { id name } }").await;
    assert!(
        !json["errors"].as_array().unwrap_or(&vec![]).is_empty(),
        "the emitted class gate rejects a caller with no Read rule"
    );
}

// A subscription's posture keeps applying per item: one stream, two
// subscribers, two item sequences.

/// The ability a `role` carries — the same three the `x-role` guard builds.
fn ability_for(role: &str) -> Arc<nest_rs_authz::Ability> {
    let mut builder = AbilityBuilder::new();
    match role {
        "admin" => {
            builder.can(Action::Read, widget::Entity);
        }
        "auditor" => {
            builder
                .can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::Id, 1))
                .fields([widget::Column::Id]);
        }
        _ => {}
    }
    Arc::new(builder.build().expect("valid test ability"))
}

/// One subscribe as `role`: the subscription, its GraphQL context seeded with `role`'s ability as
/// `/graphql`'s seed forwards it; the subscriber runs it under the same ability,
/// ambient, as the request's ability guard leaves it.
fn subscribe(app: &TestApp, role: &str) -> GqlRequest {
    let _ = app;
    GqlRequest::new("subscription { widgetFeed { id name } }").data(ability_for(role))
}

fn payload(response: nest_rs_graphql::async_graphql::Response) -> serde_json::Value {
    serde_json::to_value(response.data).expect("a subscription payload is JSON")
}

fn ids(items: &[serde_json::Value]) -> Vec<i64> {
    items
        .iter()
        .filter_map(|item| item["widgetFeed"]["id"].as_i64())
        .collect()
}

/// What the harness's socket promises: the ability it attaches is the ambient
/// one the class gate reads and the one the seed forwards to the mask, as an
/// upgrade's guards leave it.
#[tokio::test]
async fn a_socket_attaching_an_ability_reaches_a_gated_masked_subscription() {
    let app = boot().await;
    let feed = app
        .container()
        .get::<WidgetFeed>()
        .expect("the feed is a provider of the booted app");
    let mut socket = app.graphql_socket().attach(ability_for("auditor")).open();
    socket.connect().await;
    socket.subscribe("1", "subscription { widgetFeed { id name } }");
    let emit = async {
        nest_rs_testing::wait_until(std::time::Duration::from_secs(5), || {
            feed.tx.receiver_count() >= 1
        })
        .await;
        feed.tx
            .send(WidgetDto {
                id: 1,
                name: Some("ada".into()),
            })
            .expect("the subscriber is live");
    };

    let (item, ()) = tokio::join!(socket.next_item("1"), emit);

    let item = item.expect("an item");
    assert_eq!(
        item["data"]["widgetFeed"]["id"], 1,
        "the gate admitted: {item}"
    );
    assert_eq!(
        item["data"]["widgetFeed"]["name"],
        serde_json::Value::Null,
        "the mask read the auditor's ability: {item}",
    );
}

/// The load-bearing one. Two subscribers, one emitted sequence: the item the
/// auditor may not read never reaches them — it is **absent**, not delivered
/// with its fields nulled.
///
/// Items `1`, `2`, `1` are emitted, so the auditor's two are both `1`s: `2` was
/// skipped, not late.
#[tokio::test]
async fn an_item_outside_the_grant_never_reaches_that_subscriber() {
    let app = boot().await;
    let schema = nest_rs_graphql::__private::compose_schema(
        app.container().clone(),
        &nest_rs_graphql::GraphqlConfig::default(),
    );
    let feed = app
        .container()
        .get::<WidgetFeed>()
        .expect("the feed is a provider of the booted app");

    let admin = nest_rs_authz::with_ability(
        ability_for("admin"),
        schema
            .execute_stream(subscribe(&app, "admin"), None)
            .map(payload)
            .take(3)
            .collect::<Vec<_>>(),
    );
    let auditor = nest_rs_authz::with_ability(
        ability_for("auditor"),
        schema
            .execute_stream(subscribe(&app, "auditor"), None)
            .map(payload)
            .take(2)
            .collect::<Vec<_>>(),
    );
    let emit = async {
        // A stream registers its receiver on first poll: publish only once both
        // are polled, or the send lands nowhere and the test hangs.
        nest_rs_testing::wait_until(std::time::Duration::from_secs(5), || {
            feed.tx.receiver_count() >= 2
        })
        .await;
        for (id, name) in [(1, "ada"), (2, "grace"), (1, "ada")] {
            feed.tx
                .send(WidgetDto {
                    id,
                    name: Some(name.into()),
                })
                .expect("receivers are live");
        }
    };

    let (admin, auditor, ()) = tokio::join!(admin, auditor, emit);

    assert_eq!(ids(&admin), vec![1, 2, 1], "admin reads the whole feed");
    assert_eq!(
        ids(&auditor),
        vec![1, 1],
        "widget 2 is outside the auditor's grant, so it never arrives: {auditor:?}",
    );
    assert_eq!(
        auditor[0]["widgetFeed"]["name"],
        serde_json::Value::Null,
        "the item that does arrive is still field-masked: {auditor:?}",
    );
    assert_eq!(
        admin[1]["widgetFeed"]["name"], "grace",
        "the same emitted item is unmasked for the caller who may read it",
    );
}

/// A post as the feed pushes it and the queries return it: the wire carries no
/// `deleted`, and no `archived_at`.
#[derive(Clone, Debug, SimpleObject, Serialize, Deserialize)]
struct PostDto {
    id: i32,
    org_id: i32,
    title: String,
}

impl PostDto {
    fn new(id: i32, org_id: i32) -> Self {
        Self {
            id,
            org_id,
            title: format!("post {id}"),
        }
    }
}

#[injectable]
struct PostFeed {
    tx: broadcast::Sender<PostDto>,
}

impl Default for PostFeed {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(8).0,
        }
    }
}

#[resolver]
struct PostFeedResolver {
    #[inject]
    feed: Arc<PostFeed>,
}

#[operations]
impl PostFeedResolver {
    /// The rows a `Repo` read under the caller's ability returns: posts of
    /// orgs 7 and 8.
    #[query]
    #[authorize(Read, crate::post::Entity)]
    async fn posts(&self) -> GqlResult<Vec<PostDto>> {
        Ok(vec![PostDto::new(1, 7), PostDto::new(2, 8)])
    }

    #[subscription]
    #[authorize(Read, crate::post::Entity)]
    async fn post_feed(&self) -> Result<impl Stream<Item = PostDto>, GqlError> {
        let rx = self.feed.tx.subscribe();
        Ok(stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(post) => Some((post, rx)),
                Err(_) => None,
            }
        }))
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        PassGuard,
        AbilityInjector,
        EndpointGuards<GraphqlView, (PassGuard, AbilityInjector)>,
        PostFeed,
        PostFeedResolver,
    ],
)]
struct PostFeedGraphqlModule;

async fn boot_posts() -> TestApp {
    TestApp::builder()
        .module::<PostFeedGraphqlModule>()
        .build()
        .await
        .expect("the post schema boots")
}

/// The first `postFeed` item a subscriber under `ability` receives, once the
/// feed pushed `posts` (`(id, org_id)` each).
async fn first_post_pushed(ability: AbilityBuilder, posts: &[(i32, i32)]) -> serde_json::Value {
    let app = boot_posts().await;
    let schema = nest_rs_graphql::__private::compose_schema(
        app.container().clone(),
        &nest_rs_graphql::GraphqlConfig::default(),
    );
    let feed = app
        .container()
        .get::<PostFeed>()
        .expect("the feed is a provider of the booted app");
    let ability = Arc::new(ability.build().expect("valid test ability"));
    // Ambient, as the request's ability guard leaves it, and seeded into the
    // GraphQL context, as `/graphql`'s seed forwards it.
    let received = nest_rs_authz::with_ability(
        Arc::clone(&ability),
        schema
            .execute_stream(
                GqlRequest::new("subscription { postFeed { id orgId title } }").data(ability),
                None,
            )
            .map(payload)
            .take(1)
            .collect::<Vec<_>>(),
    );
    let emit = async {
        nest_rs_testing::wait_until(std::time::Duration::from_secs(5), || {
            feed.tx.receiver_count() >= 1
        })
        .await;
        for &(id, org_id) in posts {
            feed.tx
                .send(PostDto::new(id, org_id))
                .expect("the subscriber is live");
        }
    };
    let (received, ()) = tokio::join!(received, emit);
    received[0]["postFeed"].clone()
}

/// `query` run under `ability`, its data and errors as JSON.
async fn posts_read(ability: AbilityBuilder, query: &str) -> serde_json::Value {
    let app = boot_posts().await;
    let schema = nest_rs_graphql::__private::compose_schema(
        app.container().clone(),
        &nest_rs_graphql::GraphqlConfig::default(),
    );
    let ability = Arc::new(ability.build().expect("valid test ability"));
    // Ambient, as the request's ability guard leaves it, and seeded into the
    // GraphQL context, as `/graphql`'s seed forwards it.
    let response = nest_rs_authz::with_ability(
        Arc::clone(&ability),
        schema.execute(GqlRequest::new(query).data(ability)),
    )
    .await;
    serde_json::to_value(response).expect("a GraphQL response is JSON")
}

/// No SQL read a pushed item, so a relation grant decides nothing about it: the
/// subscriber's one item is the post its decided grant names, never a post of
/// another org the relation would have refused.
#[tokio::test]
async fn an_item_a_relation_alone_grants_never_reaches_the_subscriber() {
    use crate::post;

    let mut member = AbilityBuilder::new();
    member
        .can(Action::Read, post::Entity)
        .when(|p| post::in_org(&p, 7));
    member
        .can(Action::Read, post::Entity)
        .when(|p| p.eq(post::Column::Id, 99));

    let first = first_post_pushed(member, &[(1, 8), (2, 7), (99, 8)]).await;

    assert_eq!(
        first["id"], 99,
        "posts 1 and 2 rest on the relation alone: {first}"
    );
}

/// `PostDto` leaves `archived_at` out, so an item never says whether it is
/// archived: the grant reading it decides nothing, where the `None` serde gave
/// the rebuilt row admitted every item.
#[tokio::test]
async fn an_item_whose_type_leaves_the_rules_column_out_is_not_judged_on_it() {
    use crate::post;

    let mut reader = AbilityBuilder::new();
    reader
        .can(Action::Read, post::Entity)
        .when(|p| p.is_null(post::Column::ArchivedAt));
    reader
        .can(Action::Read, post::Entity)
        .when(|p| p.eq(post::Column::Id, 99));

    let first = first_post_pushed(reader, &[(1, 7), (99, 7)]).await;

    assert_eq!(
        first["id"], 99,
        "post 1 rests on a column it never carried: {first}"
    );
}

/// The read behind a query already left out what a relational denial names,
/// so the mask keeps every row it was handed.
#[tokio::test]
async fn a_relational_denial_keeps_every_row_a_query_read() {
    use crate::post;

    let mut member = AbilityBuilder::new();
    member.can(Action::Read, post::Entity);
    member
        .cannot(Action::Read, post::Entity)
        .when(|p| post::in_org(&p, 9));

    let json = posts_read(member, "{ posts { id orgId title } }").await;

    assert_eq!(
        json["data"]["posts"].as_array().map(Vec::len),
        Some(2),
        "{json}"
    );
}

/// A field grant strips `title`, which `PostDto` requires, so the mask reports
/// what it took instead of handing the value back; the rows it reports on are
/// still every row the read admitted.
#[tokio::test]
async fn a_field_masked_query_keeps_every_row_a_relational_denial_left() {
    use crate::post;

    let mut member = AbilityBuilder::new();
    member
        .can(Action::Read, post::Entity)
        .fields([post::Column::Id, post::Column::OrgId]);
    member
        .cannot(Action::Read, post::Entity)
        .when(|p| post::in_org(&p, 9));

    let json = posts_read(member, "{ posts { id orgId } }").await;

    assert!(json.get("errors").is_none(), "{json}");
    assert_eq!(
        json["data"]["posts"].as_array().map(Vec::len),
        Some(2),
        "{json}"
    );
}

/// The gate itself, at subscribe: a caller with no `Read` rule is refused before
/// any item exists — the same refusal a query gets, so the stream carries an
/// error and ends rather than opening.
#[tokio::test]
async fn a_caller_with_no_grant_is_refused_at_subscribe() {
    let app = boot().await;
    let schema = nest_rs_graphql::__private::compose_schema(
        app.container().clone(),
        &nest_rs_graphql::GraphqlConfig::default(),
    );
    let responses: Vec<_> = schema
        .execute_stream(subscribe(&app, ""), None)
        .collect()
        .await;
    let first = responses.first().expect("the refusal is delivered");
    assert!(
        !first.errors.is_empty(),
        "the class gate refuses a caller with no Read rule: {:?}",
        first.data,
    );
    assert_eq!(
        serde_json::to_value(&first.data).expect("a payload is JSON")["widgetFeed"],
        serde_json::Value::Null,
        "no item ships to a caller the gate refused",
    );
}

#[tokio::test]
async fn public_posture_skips_gate_and_mask() {
    let app = boot().await;
    let json = query(&app, "", "{ motd }").await;
    assert_eq!(json["data"]["motd"], "hello");
}

#[tokio::test]
async fn masked_reply_fails_closed_without_an_ambient_ability() {
    let wire = serde_json::json!({ "id": 1, "name": "ada" });
    let result = masked_reply::<widget::Entity>(Action::Read, wire);
    assert!(
        matches!(result, Err(MaskReplyError::NoAmbientAbility)),
        "no ambient ability must fail closed, not passthrough",
    );
}

#[tokio::test]
async fn masked_reply_strips_unpermitted_fields_and_unexposed_columns() {
    // A viewer reads only `id`: `name` masks to null, `secret` is strained out.
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .fields([widget::Column::Id]);
    let ability = Arc::new(b.build().expect("valid ability"));
    let wire = serde_json::json!({ "id": 1, "name": "ada" });

    let masked = with_ability(ability, async move {
        masked_reply::<widget::Entity>(Action::Read, wire)
    })
    .await
    .expect("masks with an ambient ability");

    assert_eq!(masked["id"], 1);
    assert_eq!(
        masked["name"],
        serde_json::Value::Null,
        "a field outside the grant is masked",
    );
    assert!(
        masked.get("secret").is_none(),
        "the server-only column is strained out",
    );
}

#[tokio::test]
async fn masked_reply_masks_every_row_of_an_array() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .fields([widget::Column::Id]);
    let ability = Arc::new(b.build().expect("valid ability"));
    let wire = serde_json::json!([
        { "id": 1, "name": "ada" },
        { "id": 2, "name": "grace" },
    ]);

    let masked = with_ability(ability, async move {
        masked_reply::<widget::Entity>(Action::Read, wire)
    })
    .await
    .expect("masks an array");

    let rows = masked.as_array().expect("array reply");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["name"], serde_json::Value::Null);
        assert!(row.get("secret").is_none());
    }
}

// `#[entity]` carries the same posture, reached through a reference a
// federation router sends rather than a field the document spells.

#[resolver]
struct EntityMaskResolver;

#[operations]
impl EntityMaskResolver {
    /// Same declaration as the `#[query]` above, on the one operation whose
    /// posture no document shows.
    #[entity]
    #[authorize(Read, widget::Entity)]
    async fn find_widget_dto_by_id(&self, id: i32) -> GqlResult<WidgetDto> {
        Ok(WidgetDto {
            id,
            name: Some("ada".into()),
        })
    }
}

#[module(
    imports = [GraphqlModule::for_root(nest_rs_graphql::GraphqlConfig {
        federation: true,
        ..Default::default()
    })],
    providers = [
        PassGuard,
        AbilityInjector,
        EndpointGuards<GraphqlView, (PassGuard, AbilityInjector)>,
        EntityMaskResolver,
    ],
)]
struct EntityMaskGraphqlModule;

async fn boot_subgraph() -> TestApp {
    TestApp::builder()
        .module::<EntityMaskGraphqlModule>()
        .build()
        .await
        .expect("the subgraph boots and mounts at /graphql")
}

/// One reference, one role — the driver every case below shares.
async fn reference(app: &TestApp, role: &str) -> serde_json::Value {
    let mut req = app.http().post("/graphql");
    if !role.is_empty() {
        req = req.header("x-role", role);
    }
    let resp = req
        .body_json(&serde_json::json!({
            "query": "query($reps: [_Any!]!) { _entities(representations: $reps) \
                      { ... on WidgetDto { id name } } }",
            "variables": { "reps": [{ "__typename": "WidgetDto", "id": 1 }] },
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    serde_json::to_value(resp.json().await).expect("a GraphQL response is JSON")
}

#[tokio::test]
async fn an_entity_answers_an_unrestricted_caller_in_full() {
    let app = boot_subgraph().await;
    let json = reference(&app, "admin").await;
    assert_eq!(json["data"]["_entities"][0]["id"], 1, "{json}");
    assert_eq!(json["data"]["_entities"][0]["name"], "ada", "{json}");
}

/// The mask runs on a reference exactly as on a query, and nothing in the
/// resolver body says so — the posture attribute is the whole mechanism.
#[tokio::test]
async fn an_entity_masks_a_field_restricted_caller() {
    let app = boot_subgraph().await;
    let json = reference(&app, "viewer").await;
    assert_eq!(json["data"]["_entities"][0]["id"], 1, "{json}");
    assert!(
        json["data"]["_entities"][0]["name"].is_null(),
        "a field grant the caller lacks is stripped from a reference too — \
         otherwise every `@key`-ed type is a way past every field grant in the \
         schema: {json}",
    );
}

/// And the class gate: a caller with no rule for the entity is refused, rather
/// than answered from a body the document never mentioned.
#[tokio::test]
async fn an_entity_refuses_a_caller_with_no_rule() {
    let app = boot_subgraph().await;
    let json = reference(&app, "stranger").await;
    assert!(
        json["data"].is_null() || json["data"]["_entities"][0].is_null(),
        "the reference is not answered: {json}",
    );
    assert!(
        json["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty()),
        "and it is refused rather than answered empty: {json}",
    );
}

/// The ability a field's own guard attaches is the one its class gate checks
/// and its mask reads: one place, the unit's.
mod ability_of_the_unit {
    use super::{WidgetDto, widget};
    use nest_rs_authz::{AbilityBuilder, Action, Read};
    use nest_rs_core::{Layer, UnitContext, injectable, module};
    use nest_rs_graphql::async_graphql::Result as GqlResult;
    use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
    use nest_rs_guards::{Denial, Guard, GuardFor};
    use nest_rs_http::async_trait;
    use nest_rs_testing::TestApp;

    use super::super::query;

    /// Attaches, at the field, an ability reading widgets' `id` alone.
    #[injectable]
    #[derive(Default)]
    struct FieldAbility;

    impl Layer for FieldAbility {}

    #[async_trait]
    impl Guard for FieldAbility {
        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let mut builder = AbilityBuilder::new();
            builder
                .can(Action::Read, widget::Entity)
                .fields([widget::Column::Id]);
            cx.attach(builder.build().expect("a valid ability"));
            Ok(())
        }
    }

    impl GuardFor<GraphqlView> for FieldAbility {}

    #[resolver]
    struct FieldAbilityResolver;

    #[operations]
    impl FieldAbilityResolver {
        #[query]
        #[authorize(Read, widget::Entity)]
        #[use_guards(FieldAbility)]
        async fn own_widget(&self) -> GqlResult<WidgetDto> {
            Ok(WidgetDto {
                id: 7,
                name: Some("ada".into()),
            })
        }
    }

    #[module(
        imports = [GraphqlModule::for_root(None)],
        providers = [FieldAbility, FieldAbilityResolver],
    )]
    struct FieldAbilityModule;

    #[tokio::test]
    async fn the_mask_reads_the_ability_the_class_gate_checked() {
        let app = TestApp::for_module::<FieldAbilityModule>()
            .await
            .expect("boots");

        let json = query(&app, "", "{ ownWidget { id name } }").await;

        assert!(json.get("errors").is_none(), "{json}");
        assert_eq!(json["data"]["ownWidget"]["id"], 7, "{json}");
        assert_eq!(
            json["data"]["ownWidget"]["name"],
            serde_json::Value::Null,
            "masked by the ability the field's guard attached: {json}",
        );
    }
}
