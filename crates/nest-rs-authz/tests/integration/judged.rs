//! Mirror of `src/judged.rs`: a wire row judged where `#[expose]`'s conversion
//! builds it, through the real decorator, on a real route and a real schema — no
//! database, every row built from a whole `Model` as a `Repo` read hands it over.

use std::sync::{Arc, Mutex, OnceLock};

use nest_rs_authz::{AbilityBuilder, Action, Read, Update};
use nest_rs_core::{Layer, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream, StreamExt};
use nest_rs_graphql::async_graphql::{
    Executor as _, Json as GqlJson, Request as GqlRequest, Result as GqlResult, SimpleObject,
};
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::{EndpointGuards, HttpView, Json, StatusCode, controller, routes};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_ws::{gateway, messages};

/// A member of an org: its name every reply reads, its email and nickname a grant
/// may withhold, its password hash no reply ever holds.
pub(crate) mod member {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Member", graphql, complex)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "members")]
    pub(crate) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub org_id: i32,
        #[expose]
        pub name: String,
        #[expose(nullable)]
        pub email: String,
        #[expose]
        pub nickname: Option<String>,
        pub password_hash: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(crate) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    /// Member `id` of org `org_id`, every column filled.
    pub(crate) fn row(id: i32, org_id: i32) -> Model {
        Model {
            id,
            org_id,
            name: format!("member {id}"),
            email: format!("member{id}@example.test"),
            nickname: Some(format!("m{id}")),
            password_hash: "$argon2id$secret".into(),
        }
    }
}

use member::Member;

/// What no reply may ever hold, whichever shape carries the row.
const WITHHELD: [&str; 3] = ["member1@example.test", "m1", "$argon2id$secret"];

/// The caller's ability, read off `x-role`: `admin` reads every column; `member`
/// reads ids and names, and its own row whole; `peer` reads org 1's ids and names;
/// `writer` updates members and reads none; `editor` updates every column and reads
/// ids and names; anyone else reads nothing.
fn ability_of(role: &str) -> nest_rs_authz::Ability {
    let mut ab = AbilityBuilder::new();
    match role {
        "admin" => {
            ab.can(Action::Read, member::Entity);
        }
        "member" => {
            ab.can(Action::Read, member::Entity).fields([
                member::Column::Id,
                member::Column::OrgId,
                member::Column::Name,
            ]);
            ab.can(Action::Read, member::Entity)
                .when(|p| p.eq(member::Column::Id, 1));
        }
        "peer" => {
            ab.can(Action::Read, member::Entity)
                .when(|p| p.eq(member::Column::OrgId, 1))
                .fields([
                    member::Column::Id,
                    member::Column::OrgId,
                    member::Column::Name,
                ]);
        }
        "writer" => {
            ab.can(Action::Update, member::Entity);
        }
        "editor" => {
            ab.can(Action::Update, member::Entity);
            ab.can(Action::Read, member::Entity).fields([
                member::Column::Id,
                member::Column::OrgId,
                member::Column::Name,
            ]);
        }
        _ => {}
    }
    ab.build().expect("valid test ability")
}

/// Attaches the ability of the role the request names, as an app's ability guard
/// attaches its caller's.
#[injectable]
#[derive(Default, Layer)]
struct RoleGuard;

#[async_trait]
impl Guard for RoleGuard {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let role = HttpView::of(cx)?
            .request()
            .header("x-role")
            .unwrap_or_default()
            .to_owned();
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(ability_of(
            &role,
        )));
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for RoleGuard {}

/// A row built in an earlier request, under that request's ability: a cache.
static CACHED: OnceLock<Mutex<Option<Member>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<Member>> {
    CACHED.get_or_init(Mutex::default)
}

#[controller(path = "/members")]
#[derive(Default)]
struct MembersController;

#[routes]
impl MembersController {
    #[get("/")]
    #[authorize(Read, member::Entity)]
    async fn list(&self) -> nest_rs_http::Result<Json<Vec<Member>>> {
        Ok(Json(
            [member::row(1, 1), member::row(2, 1)]
                .iter()
                .map(Member::from)
                .collect(),
        ))
    }

    #[get("/cache")]
    #[authorize(Read, member::Entity)]
    async fn fill(&self) -> nest_rs_http::Result<Json<Member>> {
        let row = Member::from(&member::row(1, 1));
        *cache().lock().expect("the cache") = Some(row.clone());
        Ok(Json(row))
    }

    #[get("/cached")]
    #[authorize(Read, member::Entity)]
    async fn cached(&self) -> nest_rs_http::Result<Json<Member>> {
        let row = cache().lock().expect("the cache").clone();
        Ok(Json(row.expect("filled by an earlier request")))
    }

    #[post("/1")]
    #[authorize(Update, member::Entity)]
    async fn written(&self) -> nest_rs_http::Result<Json<Member>> {
        Ok(Json(Member::from(&member::row(1, 1))))
    }
}

#[module(providers = [
    RoleGuard,
    GlobalGuards<(RoleGuard,)>,
    MembersController,
])]
struct MembersHttpModule;

async fn get_as(app: &TestApp, role: &str, path: &str) -> (StatusCode, String) {
    let resp = app.http().get(path).header("x-role", role).send().await;
    let status = resp.0.status();
    (
        status,
        resp.0.into_body().into_string().await.unwrap_or_default(),
    )
}

/// A withheld column is no value in the row: it reads `null`, its key written, and
/// a hidden column has no key at all.
#[tokio::test]
async fn a_reply_holds_what_the_grant_reads_and_null_where_it_withholds() {
    let app = TestApp::for_module::<MembersHttpModule>()
        .await
        .expect("boots");
    let (status, body) = get_as(&app, "peer", "/members").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(
        rows,
        serde_json::json!([
            { "id": 1, "org_id": 1, "name": "member 1", "email": null, "nickname": null },
            { "id": 2, "org_id": 1, "name": "member 2", "email": null, "nickname": null },
        ]),
    );

    let (_, body) = get_as(&app, "admin", "/members").await;
    assert!(body.contains("member1@example.test"), "{body}");
    assert!(
        !body.contains("password") && !body.contains("argon2"),
        "{body}"
    );
}

/// Each row is judged on its own whole row: the caller's own reads every column, a
/// peer's what the narrower grant reads.
#[tokio::test]
async fn a_conditional_grant_judges_each_row_on_its_whole_model() {
    let app = TestApp::for_module::<MembersHttpModule>()
        .await
        .expect("boots");
    let (_, body) = get_as(&app, "member", "/members").await;
    let rows: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(rows[0]["email"], "member1@example.test", "{body}");
    assert_eq!(rows[1]["email"], serde_json::Value::Null, "{body}");
    assert_eq!(rows[1]["nickname"], serde_json::Value::Null, "{body}");
}

/// A row carries who judged it: carried into another request, it answers no one.
#[tokio::test]
async fn a_row_judged_for_another_caller_answers_no_one() {
    let app = TestApp::for_module::<MembersHttpModule>()
        .await
        .expect("boots");
    let (status, _) = get_as(&app, "admin", "/members/cache").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = get_as(&app, "peer", "/members/cached").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    for withheld in WITHHELD {
        assert!(!body.contains(withheld), "{body}");
    }
    let (status, _) = get_as(&app, "admin", "/members/cached").await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "another request is another caller, whoever it is",
    );
}

/// A write's reply shows what the caller may read: a caller who may write a row
/// and not read it is refused the reply, never handed an empty row.
#[tokio::test]
async fn a_reply_the_caller_may_not_read_is_refused() {
    let app = TestApp::for_module::<MembersHttpModule>()
        .await
        .expect("boots");
    let logs = nest_rs_testing::LogCapture::install();
    let resp = app
        .http()
        .post("/members/1")
        .header("x-role", "writer")
        .send()
        .await;
    assert_eq!(resp.0.status(), StatusCode::FORBIDDEN);
    let body = resp.0.into_body().into_string().await.unwrap_or_default();
    assert!(!body.contains("member 1"), "{body}");
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a reply's row is withheld from its caller",
    );
    assert_eq!(filed.len(), 1, "one line per refused reply: {filed:#?}");
    for (field, named) in [
        ("entity", "members"),
        ("reason", "row_not_granted"),
        ("host", "MembersController"),
        ("method", "written"),
        ("verb", "POST"),
        ("path", "/members/1"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

/// A write's reply reads the caller's read grant, never the write's: a write grant
/// naming no column opens none its read grant withholds.
#[tokio::test]
async fn a_writes_reply_holds_what_its_caller_may_read_and_never_what_its_write_grant_names() {
    let app = TestApp::for_module::<MembersHttpModule>()
        .await
        .expect("boots");
    let resp = app
        .http()
        .post("/members/1")
        .header("x-role", "editor")
        .send()
        .await;
    assert_eq!(resp.0.status(), StatusCode::OK);
    let body = resp.0.into_body().into_string().await.unwrap_or_default();
    let row: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(
        row,
        serde_json::json!({
            "id": 1, "org_id": 1, "name": "member 1", "email": null, "nickname": null,
        }),
    );
}

/// A row judged for another caller is a defect of the body that carried it: its
/// line names the route as its `mounted` line does.
#[tokio::test]
async fn a_row_judged_for_another_caller_names_the_route_that_answered_it() {
    let app = TestApp::for_module::<MembersHttpModule>()
        .await
        .expect("boots");
    let (status, _) = get_as(&app, "admin", "/members/cache").await;
    assert_eq!(status, StatusCode::OK);
    let logs = nest_rs_testing::LogCapture::install();
    let (status, _) = get_as(&app, "peer", "/members/cached").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a wire row answered a caller it was not judged for",
    );
    assert_eq!(filed.len(), 1, "{filed:#?}");
    for (field, named) in [
        ("entity", "members"),
        ("reason", "judged_for_another"),
        ("host", "MembersController"),
        ("method", "cached"),
        ("verb", "GET"),
        ("path", "/members/cached"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

/// A row a job built, every exposed column, kept past the job as a cache keeps it.
static WARMED: OnceLock<Mutex<Option<Member>>> = OnceLock::new();

/// The executor a job's unit runs on: nothing here reads through it.
struct NoDatabase;

impl nest_rs_database::Executor for NoDatabase {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Member 2 of org 2, built by a job as system work.
async fn warm() {
    let row = nest_rs_database::__private::with_job_executor(Arc::new(NoDatabase), async {
        Member::from(&member::row(2, 2))
    })
    .await;
    *WARMED.get_or_init(Mutex::default).lock().expect("warmed") = Some(row);
}

#[controller(path = "/warmed")]
#[derive(Default)]
struct WarmedController;

#[routes]
impl WarmedController {
    #[get("/")]
    #[public]
    async fn warmed(&self) -> nest_rs_http::Result<Json<Member>> {
        let row = WARMED
            .get_or_init(Mutex::default)
            .lock()
            .expect("warmed")
            .clone();
        Ok(Json(row.expect("warmed by the test")))
    }
}

/// No guard attaches an ability: the boot admits the `#[public]` route.
#[module(providers = [WarmedController])]
struct WarmedHttpModule;

/// A row a job built answers no caller outside a job: a route holding no ability
/// is refused it, as one holding the caller's is, its line naming the route though
/// no layer and no unit of work runs there.
#[tokio::test]
async fn a_row_a_job_built_answers_no_caller() {
    warm().await;
    let app = TestApp::for_module::<WarmedHttpModule>()
        .await
        .expect("boots");
    let logs = nest_rs_testing::LogCapture::install();
    let (status, body) = get_as(&app, "", "/warmed").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        !body.contains("member2@example.test") && !body.contains("member 2"),
        "{body}"
    );
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a wire row answered a caller it was not judged for",
    );
    assert_eq!(filed.len(), 1, "{filed:#?}");
    for (field, named) in [
        ("entity", "members"),
        ("reason", "judged_for_another"),
        ("host", "WarmedController"),
        ("method", "warmed"),
        ("verb", "GET"),
        ("path", "/warmed"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

#[gateway(path = "/ws/members")]
#[derive(Default)]
struct MembersGateway;

#[messages]
impl MembersGateway {
    #[subscribe_message("written")]
    #[authorize(Update, member::Entity)]
    async fn written(&self) -> Member {
        Member::from(&member::row(1, 1))
    }
}

#[module(
    imports = [nest_rs_ws::WsModule],
    providers = [RoleGuard, GlobalGuards<(RoleGuard,)>, MembersGateway],
)]
struct MembersWsModule;

#[mcp(path = "/mcp/members")]
#[derive(Clone, Default)]
struct MembersTool;

#[tools]
impl MembersTool {
    /// The member a write answers.
    #[tool]
    #[authorize(Update, member::Entity)]
    async fn written(&self) -> Result<nest_rs_mcp::Json<Member>, McpError> {
        Ok(nest_rs_mcp::Json(Member::from(&member::row(1, 1))))
    }
}

#[module(providers = [RoleGuard, GlobalGuards<(RoleGuard,)>, MembersTool])]
struct MembersMcpModule;

/// A message's reply the caller may not read is refused as a route's is, its line
/// naming the message as its `mounted` line does.
#[tokio::test]
async fn a_message_reply_the_caller_may_not_read_is_refused_naming_its_message() {
    let ws = TestApp::builder()
        .module::<MembersWsModule>()
        .build_ws()
        .await
        .expect("the gateway boots");
    let logs = nest_rs_testing::LogCapture::install();
    let frame = crate::ws::message_as(&ws, "/ws/members", Some("writer"), "written").await;
    ws.shutdown().await.expect("the transport stops cleanly");
    let frame = frame.to_string();
    assert!(frame.contains("FORBIDDEN"), "{frame}");
    assert!(!frame.contains("member 1"), "{frame}");
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a reply's row is withheld from its caller",
    );
    assert_eq!(filed.len(), 1, "{filed:#?}");
    for (field, named) in [
        ("entity", "members"),
        ("reason", "row_not_granted"),
        ("host", "MembersGateway"),
        ("method", "written"),
        ("path", "/ws/members"),
        ("event", "written"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

/// A tool's answer the caller may not read is refused as a route's is, its line
/// naming the tool as its `mounted` line does.
#[tokio::test]
async fn a_tool_answer_the_caller_may_not_read_is_refused_naming_its_tool() {
    let app = TestApp::for_module::<MembersMcpModule>()
        .await
        .expect("boots");
    let logs = nest_rs_testing::LogCapture::install();
    let answer = nest_rs_testing::mcp::call_tool_statelessly(
        app.http(),
        "/mcp/members",
        "written",
        None,
        &[("x-role", "writer")],
        serde_json::json!({}),
    )
    .await;
    assert!(answer.contains("FORBIDDEN"), "{answer}");
    assert!(!answer.contains("member 1"), "{answer}");
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a reply's row is withheld from its caller",
    );
    assert_eq!(filed.len(), 1, "{filed:#?}");
    for (field, named) in [
        ("entity", "members"),
        ("reason", "row_not_granted"),
        ("host", "MembersTool"),
        ("method", "written"),
        ("path", "/mcp/members"),
        ("tool", "written"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

/// What a body builds around a wire row holds only what the row holds.
#[derive(SimpleObject)]
#[graphql(name = "Holder")]
struct Holder {
    member: Member,
    note: String,
}

/// A member's columns copied into an object of the body's own.
#[derive(SimpleObject)]
struct Flattened {
    #[graphql(flatten)]
    member: Member,
    rank: i32,
}

/// A push of the body's own holding the member it is about.
#[derive(SimpleObject)]
struct MemberMoved {
    member: Member,
}

/// Member 1 of org 1 and member 2 of org 2, which the peer may not read.
fn both_orgs() -> [member::Model; 2] {
    [member::row(1, 1), member::row(2, 2)]
}

#[resolver]
#[derive(Default)]
struct MembersResolver;

#[operations]
impl MembersResolver {
    #[query]
    #[authorize(Read, member::Entity)]
    async fn members(&self) -> Vec<Member> {
        [member::row(1, 1), member::row(2, 1)]
            .iter()
            .map(Member::from)
            .collect()
    }

    /// A list a body built by hand, holding a row the peer's grants refuse.
    #[query]
    #[authorize(Read, member::Entity)]
    async fn everyone(&self) -> Vec<Member> {
        both_orgs().iter().map(Member::from).collect()
    }

    /// A scalar the body builds: the rows serialize whole.
    #[query]
    #[authorize(Read, member::Entity)]
    async fn members_json(&self) -> GqlJson<Vec<Member>> {
        GqlJson([member::row(1, 1)].iter().map(Member::from).collect())
    }

    #[query]
    #[authorize(Read, member::Entity)]
    async fn holder(&self) -> Holder {
        Holder {
            member: Member::from(&member::row(1, 1)),
            note: "held".into(),
        }
    }

    #[query]
    #[authorize(Read, member::Entity)]
    async fn flattened(&self) -> Flattened {
        Flattened {
            member: Member::from(&member::row(1, 1)),
            rank: 1,
        }
    }

    #[query]
    #[authorize(Read, member::Entity)]
    async fn cached(&self) -> Option<Member> {
        cache().lock().expect("the cache").clone()
    }

    /// A scalar the body builds around the row a cache kept.
    #[query]
    #[authorize(Read, member::Entity)]
    async fn cached_json(&self) -> GqlJson<Option<Member>> {
        GqlJson(cache().lock().expect("the cache").clone())
    }

    #[mutation]
    #[authorize(Update, member::Entity)]
    async fn write_member(&self) -> Member {
        Member::from(&member::row(1, 1))
    }

    #[subscription]
    #[authorize(Read, member::Entity)]
    async fn member_feed(&self) -> impl Stream<Item = Member> {
        stream::iter(both_orgs()).map(|row| Member::from(&row))
    }

    #[subscription]
    #[authorize(Read, member::Entity)]
    async fn optional_feed(&self) -> impl Stream<Item = Option<Member>> {
        stream::iter(both_orgs()).map(|row| Some(Member::from(&row)))
    }

    #[subscription]
    #[authorize(Read, member::Entity)]
    async fn fallible_feed(&self) -> impl Stream<Item = GqlResult<Member>> {
        stream::iter(both_orgs()).map(|row| Ok(Member::from(&row)))
    }

    #[subscription]
    #[authorize(Read, member::Entity)]
    async fn moves(&self) -> impl Stream<Item = MemberMoved> {
        stream::iter(both_orgs()).map(|row| MemberMoved {
            member: Member::from(&row),
        })
    }

    /// A scalar a field resolver derives from its parent's columns.
    #[field_resolver]
    async fn email_domain(&self, parent: &Member) -> Option<String> {
        parent
            .email
            .as_deref()
            .and_then(|email| email.split('@').nth(1))
            .map(str::to_owned)
    }

    /// A push carrying a row a publisher built, under its own ability.
    #[subscription]
    #[authorize(Read, member::Entity)]
    async fn published_feed(&self) -> impl Stream<Item = Member> {
        let published = published();
        stream::iter([published])
    }
}

/// A row built under the admin's ability, as a publisher hands it to a bus.
fn published() -> Member {
    let ability = Arc::new(ability_of("admin"));
    futures_executor_block(nest_rs_authz::__private::install_ability(ability, async {
        Member::from(&member::row(1, 1))
    }))
}

/// Runs a future holding no I/O to its end on this thread.
fn futures_executor_block<F: std::future::Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    loop {
        if let std::task::Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [
        RoleGuard,
        EndpointGuards<GraphqlView, (RoleGuard,)>,
        MembersResolver,
    ],
)]
struct MembersGraphqlModule;

async fn graphql_as(app: &TestApp, role: &str, query: &str) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .header("x-role", role)
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    serde_json::to_value(resp.json().await).expect("a GraphQL response is JSON")
}

/// GraphQL serves what HTTP does: a withheld column reads `null`, with no error.
#[tokio::test]
async fn a_withheld_column_reads_null_on_graphql() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    let read = graphql_as(&app, "peer", "{ members { id name email nickname } }").await;
    assert_eq!(
        read,
        serde_json::json!({ "data": { "members": [
            { "id": 1, "name": "member 1", "email": null, "nickname": null },
            { "id": 2, "name": "member 2", "email": null, "nickname": null },
        ] } }),
    );
    let read = graphql_as(&app, "member", "{ members { email } }").await;
    assert_eq!(
        read["data"]["members"],
        serde_json::json!([{ "email": "member1@example.test" }, { "email": null }]),
        "{read}",
    );
}

/// The schema says what a reply can hold: a column a grant may withhold is
/// nullable, every other is not.
#[tokio::test]
async fn the_schema_says_which_columns_a_grant_may_withhold() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    let read = graphql_as(
        &app,
        "admin",
        r#"{ __type(name: "Member") { fields { name type { kind } } } }"#,
    )
    .await;
    let kinds: std::collections::BTreeMap<String, String> = read["data"]["__type"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .map(|field| {
            (
                field["name"].as_str().expect("a name").to_owned(),
                field["type"]["kind"].as_str().expect("a kind").to_owned(),
            )
        })
        .collect();
    assert_eq!(kinds["id"], "NON_NULL", "{read}");
    assert_eq!(kinds["name"], "NON_NULL", "{read}");
    assert_eq!(kinds["email"], "SCALAR", "{read}");
    assert_eq!(kinds["nickname"], "SCALAR", "{read}");
    assert!(!kinds.contains_key("passwordHash"), "{read}");
}

/// Whatever shape a body puts the row in — a scalar it serializes, an object
/// holding it, an object copying its columns — the row holds nothing the grant
/// withholds, so nothing ships it.
#[tokio::test]
async fn every_shape_holding_a_row_holds_only_what_the_row_holds() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    for selection in [
        "{ membersJson }",
        "{ holder { member { id email nickname } note } }",
        "{ flattened { id email nickname rank } }",
    ] {
        let read = graphql_as(&app, "peer", selection).await;
        assert!(read.get("errors").is_none(), "{selection}: {read}");
        for withheld in WITHHELD {
            assert!(!read.to_string().contains(withheld), "{selection}: {read}");
        }
        let read = graphql_as(&app, "admin", selection).await;
        assert!(
            read.to_string().contains("member1@example.test"),
            "{selection}: {read}"
        );
    }
}

/// A row judged for another caller resolves for no one, whatever it selects.
#[tokio::test]
async fn a_row_judged_for_another_caller_resolves_for_no_one() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    *cache().lock().expect("the cache") = Some(published());
    let read = graphql_as(&app, "admin", "{ cached { id name } }").await;
    assert_eq!(read["data"]["cached"], serde_json::Value::Null, "{read}");
    assert_eq!(
        read["errors"][0]["extensions"]["code"], "INTERNAL",
        "{read}"
    );
    assert!(!read.to_string().contains("member 1"), "{read}");
}

/// A column's refusal names its root field as the field's `mounted` line does, by
/// its host, its method and its schema name, never the alias a client wrote.
#[tokio::test]
async fn a_columns_refusal_names_its_root_field_never_the_clients_alias() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    *cache().lock().expect("the cache") = Some(published());
    let logs = nest_rs_testing::LogCapture::install();
    let read = graphql_as(
        &app,
        "admin",
        "{ chosen_by_the_client: cached { id name } }",
    )
    .await;
    assert_eq!(
        read["errors"][0]["extensions"]["code"], "INTERNAL",
        "{read}"
    );
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a wire row answered a caller it was not judged for",
    );
    assert_eq!(filed.len(), 1, "{filed:#?}");
    assert!(
        !format!("{filed:?}").contains("chosen_by_the_client"),
        "{filed:#?}"
    );
    for (field, named) in [
        ("entity", "members"),
        ("reason", "judged_for_another"),
        ("host", "MembersResolver"),
        ("method", "cached"),
        ("kind", "query"),
        ("field", "cached"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

/// A row inside a `Json` scalar, which async-graphql serializes itself, refuses as a
/// column does: its root field answers the refusal rather than a `null` no error
/// explains, and its line names that field as its `mounted` line does.
#[tokio::test]
async fn a_json_scalars_refusal_is_answered_and_names_its_root_field() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    *cache().lock().expect("the cache") = Some(published());
    let logs = nest_rs_testing::LogCapture::install();
    let read = graphql_as(&app, "admin", "{ chosen_by_the_client: cachedJson }").await;
    assert_eq!(read["data"], serde_json::Value::Null, "{read}");
    assert_eq!(
        read["errors"][0]["extensions"]["code"], "INTERNAL",
        "{read}"
    );
    assert!(!read.to_string().contains("member 1"), "{read}");
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a wire row answered a caller it was not judged for",
    );
    assert_eq!(filed.len(), 1, "{filed:#?}");
    for (field, named) in [
        ("entity", "members"),
        ("reason", "judged_for_another"),
        ("host", "MembersResolver"),
        ("method", "cached_json"),
        ("kind", "query"),
        ("field", "cachedJson"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

/// A field resolver reads its parent's columns: a parent judged for another caller
/// hands it nothing to derive from, as its own columns resolve for no one.
#[tokio::test]
async fn a_field_resolver_under_a_row_judged_for_another_caller_answers_nothing() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    *cache().lock().expect("the cache") = Some(published());
    let logs = nest_rs_testing::LogCapture::install();
    let read = graphql_as(&app, "peer", "{ cached { emailDomain } }").await;
    assert!(!read.to_string().contains("example.test"), "{read}");
    assert_eq!(
        read["data"]["cached"]["emailDomain"],
        serde_json::Value::Null,
        "{read}"
    );
    assert_eq!(
        read["errors"][0]["extensions"]["code"], "INTERNAL",
        "{read}"
    );
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a wire row answered a caller it was not judged for",
    );
    assert_eq!(filed.len(), 1, "{filed:#?}");
    for (field, named) in [
        ("entity", "members"),
        ("reason", "judged_for_another"),
        ("host", "MembersResolver"),
        ("method", "email_domain"),
        ("field", "emailDomain"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }

    let read = graphql_as(&app, "admin", "{ members { emailDomain } }").await;
    assert_eq!(
        read["data"]["members"][0]["emailDomain"], "example.test",
        "a parent judged for its caller hands its resolver what it holds: {read}"
    );
}

/// A mutation's reply reads its caller's read grant, as a route's does: a column
/// the write grant opens and the read grant withholds reads `null`.
#[tokio::test]
async fn a_mutations_reply_holds_what_its_caller_may_read_and_never_what_its_write_grant_names() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    let read = graphql_as(
        &app,
        "editor",
        "mutation { writeMember { id name email nickname } }",
    )
    .await;
    assert_eq!(
        read,
        serde_json::json!({ "data": { "writeMember": {
            "id": 1, "name": "member 1", "email": null, "nickname": null,
        } } }),
    );
}

/// A row a caller's grants refuse answers its key nowhere but a write's reply: a
/// list a body built holding one fails the whole reply closed, its key with it.
#[tokio::test]
async fn a_query_holding_a_row_its_caller_may_not_read_answers_not_even_its_key() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    let read = graphql_as(&app, "peer", "{ everyone { id } }").await;
    assert_eq!(read["data"], serde_json::Value::Null, "{read}");
    assert_eq!(
        read["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{read}"
    );
    assert!(!read.to_string().contains(r#""id":2"#), "{read}");
    let read = graphql_as(&app, "admin", "{ everyone { id } }").await;
    assert_eq!(
        read,
        serde_json::json!({ "data": { "everyone": [{ "id": 1 }, { "id": 2 }] } })
    );
}

/// A write's reply answers its key to a caller who may not read the row, and
/// nothing else: the field selecting more is refused, and its write rolled back.
#[tokio::test]
async fn a_writes_refused_row_answers_its_key_and_nothing_else() {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    let read = graphql_as(&app, "writer", "mutation { writeMember { id } }").await;
    assert_eq!(
        read,
        serde_json::json!({ "data": { "writeMember": { "id": 1 } } })
    );
    let read = graphql_as(&app, "writer", "mutation { writeMember { id name } }").await;
    assert_eq!(
        read["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{read}"
    );
    assert!(!read.to_string().contains("member 1"), "{read}");
}

/// Every item `subscription` streams to `role`, the stream's end ending the read.
async fn items_to(role: &str, subscription: &str) -> Vec<serde_json::Value> {
    let app = TestApp::for_module::<MembersGraphqlModule>()
        .await
        .expect("boots");
    let schema = nest_rs_graphql::__private::compose_schema(
        app.container().clone(),
        &nest_rs_graphql::GraphqlConfig::default(),
    )
    .expect("the app's boot sealed its reachability set");
    nest_rs_authz::__private::install_ability(
        Arc::new(ability_of(role)),
        schema
            .execute_stream(GqlRequest::new(subscription), None)
            .map(|response| serde_json::to_value(response).expect("JSON"))
            .collect::<Vec<_>>(),
    )
    .await
}

/// A push is judged for each subscriber where its stream builds the row: what its
/// grant withholds reads `null`, and a row it may not read never arrives.
#[tokio::test]
async fn a_push_is_judged_for_each_subscriber() {
    let items = items_to("peer", "subscription { memberFeed { id email } }").await;
    assert_eq!(
        items,
        vec![serde_json::json!({ "data": { "memberFeed": { "id": 1, "email": null } } })],
        "member 2 is of org 2, which the peer may not read",
    );
    let items = items_to("admin", "subscription { memberFeed { id email } }").await;
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(
        items[0]["data"]["memberFeed"]["email"],
        "member1@example.test"
    );
}

/// An item holding a row inside an `Option` or a `Result` leaves the stream as a
/// bare row does: the subscriber never learns the key of a push it may not read.
#[tokio::test]
async fn a_wrapped_row_its_subscriber_may_not_read_leaves_the_stream() {
    let feeds = [
        ("subscription { optionalFeed { id } }", "optionalFeed"),
        ("subscription { fallibleFeed { id } }", "fallibleFeed"),
    ];
    let mut read = Vec::new();
    let mut kept = Vec::new();
    for (feed, field) in feeds {
        read.push(items_to("peer", feed).await);
        kept.push(vec![serde_json::json!({ "data": { field: { "id": 1 } } })]);
        assert_eq!(items_to("admin", feed).await.len(), 2, "{feed}");
    }
    assert_eq!(
        read, kept,
        "member 2 is of org 2, which the peer may not read"
    );
}

/// An item of the body's own holding such a row cannot be told from one holding
/// none: it arrives, and the row it holds answers no column, its key included.
#[tokio::test]
async fn an_object_holding_a_row_its_subscriber_may_not_read_answers_not_even_its_key() {
    let items = items_to("peer", "subscription { moves { member { id } } }").await;
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(
        items[0],
        serde_json::json!({ "data": { "moves": { "member": { "id": 1 } } } })
    );
    assert_eq!(
        items[1]["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{items:?}"
    );
    assert!(
        !serde_json::to_string(&items[1])
            .expect("JSON")
            .contains(r#""id":2"#),
        "{items:?}"
    );
}

/// A row a publisher built answers no subscriber, the publisher's own grants
/// included: the item leaves the stream.
#[tokio::test]
async fn a_row_a_publisher_built_reaches_no_subscriber() {
    let logs = nest_rs_testing::LogCapture::install();
    for role in ["peer", "admin"] {
        let items = items_to(role, "subscription { publishedFeed { id name } }").await;
        assert_eq!(items, Vec::<serde_json::Value>::new(), "{role}");
    }
    let filed = logs.find(
        nest_rs_authz::TARGET,
        "a wire row answered a caller it was not judged for",
    );
    assert_eq!(filed.len(), 2, "one line per dropped item: {filed:#?}");
    for (field, named) in [
        ("host", "MembersResolver"),
        ("method", "published_feed"),
        ("kind", "subscription"),
        ("field", "publishedFeed"),
        ("reason", "judged_for_another"),
    ] {
        assert_eq!(filed[0].field(field).as_deref(), Some(named), "{field}");
    }
}

/// A grant a reply reads may withhold only a column its wire type answers `null`:
/// any other is refused when the ability builds, naming the column and the key.
#[test]
fn a_reply_grant_withholding_a_column_no_null_answers_fails_the_build() {
    let mut ab = AbilityBuilder::new();
    ab.can(Action::Read, member::Entity)
        .fields([member::Column::Id, member::Column::OrgId]);
    let Err(refused) = ab.build() else {
        panic!("`name` cannot answer null");
    };
    let said = refused.to_string();
    assert!(
        said.contains("`name`") && said.contains("#[expose(nullable)]"),
        "{said}"
    );

    let mut ab = AbilityBuilder::new();
    ab.can(Action::Update, member::Entity)
        .fields([member::Column::Email]);
    ab.can(Action::Read, member::Entity).fields([
        member::Column::Id,
        member::Column::OrgId,
        member::Column::Name,
    ]);
    ab.build()
        .expect("a write's columns and a reply's nullable ones are left out freely");
}
