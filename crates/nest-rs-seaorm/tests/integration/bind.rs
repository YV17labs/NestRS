//! `src/bind.rs`: `#[authorize(A, bind = S)]` and `#[public(A, bind = S)]` on a
//! `#[resource]`, over a service answering each `Access` itself, so what the edge
//! says of each is pinned with no database. The load's own SQL is
//! `tests/e2e/mcp.rs`'s.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action, Read};
use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
use nest_rs_database::{Executor, OpenUnit, Settlement, Unhonoured, UnitOfWork, UnitSpec};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{EndpointGuards, HttpView};
use nest_rs_mcp::{Json, McpAccess, McpError, McpModule, McpView, mcp, tools};
use nest_rs_seaorm::{Access, Authorized, CrudService};
use nest_rs_testing::mcp::{
    call_method, call_method_with, open_session, open_session_with, result,
};
use nest_rs_testing::{LogCapture, TestApp};
use sea_orm::DbErr;
use serde_json::{Value, json};
use uuid::Uuid;

mod note {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Note")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "bound_notes")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: Option<String>,
        #[expose]
        pub body: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// The id whose row the service finds, the one it denies, the one it fails on;
/// any other is missing.
const FOUND: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001);
const DENIED: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0002);
const FAILING: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0003);
const MISSING: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0004);
/// The id whose statement Postgres cancels at its bound.
const CANCELLED: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0005);
/// A v4 id, which no bound id is.
const NOT_V7: &str = "2e2f1c6e-7a9b-4a7e-9d6b-1f0e5c1d2a3b";

/// How many times the service loaded a row, across this file's serial tests.
static LOADS: AtomicUsize = AtomicUsize::new(0);
/// How many times the body ran.
static BODIES: AtomicUsize = AtomicUsize::new(0);
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[injectable]
#[derive(Default)]
struct NotesService;

#[async_trait]
impl CrudService for NotesService {
    type Entity = note::Entity;

    async fn access(&self, action: Action, id: Uuid) -> Result<Access<note::Model>, DbErr> {
        assert_eq!(
            action,
            Action::Read,
            "the attribute's action reaches the load"
        );
        LOADS.fetch_add(1, Ordering::SeqCst);
        match id {
            FOUND => Ok(Access::Found(note::Model {
                id,
                title: Some("seen".to_owned()),
                body: Some("masked away".to_owned()),
            })),
            DENIED => Ok(Access::Denied),
            FAILING => Err(DbErr::Custom(
                "connection to db.internal refused".to_owned(),
            )),
            CANCELLED => Err(crate::error::refused(
                "57014",
                "canceling statement due to statement timeout",
            )),
            _ => Ok(Access::Missing),
        }
    }
}

/// Grants the caller what its `x-role` header says: `reader` reads every note's
/// id and title, anyone else nothing.
#[injectable]
#[derive(Default, Layer)]
struct RoleGrants;

#[async_trait]
impl Guard for RoleGrants {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("caller");
        let role = HttpView::of(cx)?
            .request()
            .header("x-role")
            .map(str::to_owned);
        let mut ability = AbilityBuilder::new();
        if role.as_deref() == Some("reader") {
            ability
                .can(Action::Read, note::Entity)
                .fields([note::Column::Id, note::Column::Title]);
        }
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            ability.build().expect("a valid rule set"),
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for RoleGrants {}

const PATH: &str = "/mcp/bound";

#[mcp(path = "/mcp/bound")]
#[derive(Clone, Default)]
struct BoundTools;

#[tools]
impl BoundTools {
    /// A note, loaded and authorized.
    #[resource(template = "note://{id}")]
    #[authorize(Read, bind = NotesService)]
    async fn note(&self, note: Authorized<Read, note::Entity>) -> Result<Json<NoteDto>, McpError> {
        BODIES.fetch_add(1, Ordering::SeqCst);
        Ok(Json(NoteDto::from(note.into_inner())))
    }

    /// The same load, its entity written and its answer's `Option` kept.
    #[resource(template = "written://{id}")]
    #[authorize(Read, note::Entity, bind = NotesService)]
    async fn written(
        &self,
        note: Authorized<Read, note::Entity>,
    ) -> Result<Option<Json<NoteDto>>, McpError> {
        BODIES.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Json(NoteDto::from(note.into_inner()))))
    }

    /// A reply leaving out a column its entity requires, which a mask cannot read
    /// back.
    #[resource(template = "narrow://{id}")]
    #[authorize(Read, bind = NotesService)]
    async fn narrow(
        &self,
        note: Authorized<Read, note::Entity>,
    ) -> Result<Json<TitleDto>, McpError> {
        BODIES.fetch_add(1, Ordering::SeqCst);
        Ok(Json(TitleDto::from(note.into_inner())))
    }

    /// The same reply, the projection declared as the field restriction.
    #[resource(template = "projected://{id}")]
    #[authorize(Read, bind = NotesService, unmasked)]
    async fn projected(
        &self,
        note: Authorized<Read, note::Entity>,
    ) -> Result<Json<TitleDto>, McpError> {
        BODIES.fetch_add(1, Ordering::SeqCst);
        Ok(Json(TitleDto::from(note.into_inner())))
    }
}

#[nest_rs_core::input]
struct NoteDto {
    id: Uuid,
    title: Option<String>,
    body: Option<String>,
}

#[nest_rs_core::input]
struct TitleDto {
    title: Option<String>,
}

impl From<note::Model> for TitleDto {
    fn from(note: note::Model) -> Self {
        Self { title: note.title }
    }
}

impl From<note::Model> for NoteDto {
    fn from(note: note::Model) -> Self {
        Self {
            id: note.id,
            title: note.title,
            body: note.body,
        }
    }
}

#[module(providers = [NotesService, RoleGrants, BoundTools, EndpointGuards<McpView, (RoleGrants,)>])]
struct BoundModule;

/// A binding whose every unit commits, on no database.
struct Committing;

struct CommittingUnit;

impl UnitOfWork for Committing {
    fn open(&self, _spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        Ok(Box::new(CommittingUnit))
    }

    fn settle_net(&self) -> std::time::Duration {
        std::time::Duration::from_secs(16)
    }
}

impl OpenUnit for CommittingUnit {
    fn executor(&self) -> Arc<dyn Executor> {
        Arc::new(Handle)
    }

    fn settle(
        self: Box<Self>,
        _ok: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Settlement> + Send>> {
        Box::pin(std::future::ready(Settlement::Settled))
    }
}

struct Handle;

impl Executor for Handle {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

async fn app() -> TestApp {
    TestApp::builder()
        .module::<BoundModule>()
        .provide_dyn::<dyn UnitOfWork>(Arc::new(Committing))
        .build()
        .await
        .expect("a host binding a resource boots on its binding")
}

/// What `role` reads at `uri`, and how many loads and bodies it took.
async fn read(app: &TestApp, role: &str, uri: &str) -> (Value, usize, usize) {
    LOADS.store(0, Ordering::SeqCst);
    BODIES.store(0, Ordering::SeqCst);
    let session = open_session_with(app.http(), PATH, None, &[("x-role", role)], json!({})).await;
    let body = call_method_with(
        app.http(),
        PATH,
        &session,
        None,
        &[("x-role", role)],
        "resources/read",
        json!({ "uri": uri }),
    )
    .await;
    (
        result(&body),
        LOADS.load(Ordering::SeqCst),
        BODIES.load(Ordering::SeqCst),
    )
}

fn not_found(answer: &Value) {
    assert_eq!(answer["error"]["message"], "Resource not found", "{answer}");
    assert_eq!(answer["error"]["data"]["code"], "NOT_FOUND", "{answer}");
}

#[tokio::test]
async fn a_row_the_load_finds_arrives_loaded_and_its_reply_is_masked() {
    let _serial = SERIAL.lock().await;
    let app = app().await;
    for uri in [format!("note://{FOUND}"), format!("written://{FOUND}")] {
        let (answer, loads, bodies) = read(&app, "reader", &uri).await;
        assert_eq!((loads, bodies), (1, 1), "{answer}");
        let text = answer["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("a text body: {answer}"));
        let note: Value = serde_json::from_str(text).expect("a JSON body");
        assert_eq!(note["title"], "seen", "{note}");
        assert_eq!(
            note["body"],
            Value::Null,
            "the mask strips what no grant reads: {note}"
        );
    }
}

/// A reader cannot tell a row it was refused from a row that does not exist: the
/// resource's one answer, and the body never runs.
#[tokio::test]
async fn denied_and_missing_answer_the_resources_not_found_without_the_body() {
    let _serial = SERIAL.lock().await;
    let app = app().await;
    for id in [DENIED, MISSING] {
        let (answer, loads, bodies) = read(&app, "reader", &format!("note://{id}")).await;
        not_found(&answer);
        assert_eq!((loads, bodies), (1, 0), "{answer}");
    }
}

/// HTTP's `400`: a template read has no such class, and the one answer stands.
#[tokio::test]
async fn an_id_that_is_no_v7_is_not_found_before_anything_is_read() {
    let _serial = SERIAL.lock().await;
    let app = app().await;
    for uri in [format!("note://{NOT_V7}"), "note://not-a-uuid".to_owned()] {
        let (answer, loads, bodies) = read(&app, "reader", &uri).await;
        not_found(&answer);
        assert_eq!((loads, bodies), (0, 0), "{answer}");
    }
}

#[tokio::test]
async fn the_class_gate_refuses_before_the_load_runs() {
    let _serial = SERIAL.lock().await;
    let app = app().await;
    let (answer, loads, bodies) = read(&app, "nobody", &format!("note://{FOUND}")).await;
    assert_eq!(answer["error"]["data"]["code"], "FORBIDDEN", "{answer}");
    assert_eq!((loads, bodies), (0, 0), "a refused caller pays for no read");
}

#[tokio::test]
async fn a_load_that_fails_is_answered_opaquely_and_filed_with_its_chain() {
    let _serial = SERIAL.lock().await;
    let app = app().await;
    let logs = LogCapture::install();
    let (answer, loads, bodies) = read(&app, "reader", &format!("note://{FAILING}")).await;
    assert_eq!(answer["error"]["data"]["code"], "INTERNAL", "{answer}");
    assert!(!answer.to_string().contains("db.internal"), "{answer}");
    assert_eq!((loads, bodies), (1, 0));
    let filed = logs.expect_one("nest_rs::mcp", "mcp operation failed");
    assert_eq!(filed.level, "error");
}

/// A statement cancelled at its bound is the server unable to serve in time, the
/// class a route's bound load answers, whatever the row.
#[tokio::test]
async fn a_statement_cancelled_at_its_bound_answers_unavailable_filed_at_warn() {
    let _serial = SERIAL.lock().await;
    let app = app().await;
    let logs = LogCapture::install();
    let (answer, loads, bodies) = read(&app, "reader", &format!("note://{CANCELLED}")).await;
    assert_eq!(answer["error"]["data"]["code"], "UNAVAILABLE", "{answer}");
    assert_eq!((loads, bodies), (1, 0));
    let filed = logs.expect_one("nest_rs::mcp", "mcp operation unavailable");
    assert_eq!(filed.level, "warn");
    assert!(
        filed
            .field("error")
            .is_some_and(|error| error.contains("57014")),
        "{:?}",
        filed.fields,
    );
}

/// As on every edge, a body a mask cannot read back ships nothing, its reason
/// filed; `unmasked` is how a resource says the projection is deliberate.
#[tokio::test]
async fn a_reply_narrower_than_its_entity_fails_closed_unless_it_declares_unmasked() {
    let _serial = SERIAL.lock().await;
    let app = app().await;
    let logs = LogCapture::install();
    let (answer, loads, bodies) = read(&app, "reader", &format!("narrow://{FOUND}")).await;
    assert_eq!(answer["error"]["data"]["code"], "INTERNAL", "{answer}");
    assert!(!answer.to_string().contains("seen"), "{answer}");
    assert_eq!((loads, bodies), (1, 1), "{answer}");
    let filed = logs.expect_one("nest_rs::authz", "response masking failed");
    assert_eq!(
        filed.field("reason").as_deref(),
        Some("irreconcilable_wire_value"),
    );

    let (answer, loads, bodies) = read(&app, "reader", &format!("projected://{FOUND}")).await;
    assert_eq!((loads, bodies), (1, 1), "{answer}");
    let text = answer["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a text body: {answer}"));
    assert_eq!(text, r#"{"title":"seen"}"#, "{answer}");
}

#[module(providers = [RoleGrants, BoundTools, EndpointGuards<McpView, (RoleGrants,)>])]
struct UnimportedServiceModule;

#[tokio::test]
async fn a_bound_service_no_module_provides_refuses_the_boot() {
    let Err(refused) = TestApp::builder()
        .module::<UnimportedServiceModule>()
        .provide_dyn::<dyn UnitOfWork>(Arc::new(Committing))
        .build()
        .await
    else {
        panic!("the load could never resolve its service");
    };
    let said = format!("{refused:#}");
    assert!(said.contains("NotesService"), "{said}");
}

#[module(providers = [NotesService, RoleGrants, BoundTools, EndpointGuards<McpView, (RoleGrants,)>])]
struct NoBindingModule;

#[tokio::test]
async fn a_bound_resource_with_no_unit_of_work_refuses_the_boot() {
    let Err(refused) = TestApp::for_module::<NoBindingModule>().await else {
        panic!("the load could never reach the database");
    };
    let said = format!("{refused:#}");
    assert!(said.contains("SeaOrmDatabaseModule"), "{said}");
}

#[injectable]
#[derive(Default)]
struct VisitorGrants;

impl AbilityFactory for VisitorGrants {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, _ab: &mut AbilityBuilder) {}

    fn define_visitor(&self, ab: &mut AbilityBuilder) {
        ab.can(Action::Read, note::Entity)
            .fields([note::Column::Id, note::Column::Title]);
    }
}

type VisitorGuard = AbilityGuard<VisitorGrants>;

const VISITED_PATH: &str = "/mcp/visited";

#[mcp(path = "/mcp/visited")]
#[derive(Clone, Default)]
struct VisitedTools;

#[tools]
impl VisitedTools {
    /// A note a caller with no credential reads, loaded and masked by its grant.
    #[resource(template = "note://{id}")]
    #[public(Read, bind = NotesService)]
    #[use_guards(VisitorGuard)]
    async fn note(&self, note: Authorized<Read, note::Entity>) -> Result<Json<NoteDto>, McpError> {
        BODIES.fetch_add(1, Ordering::SeqCst);
        Ok(Json(NoteDto::from(note.into_inner())))
    }
}

#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [NotesService, VisitorGrants, VisitorGuard, VisitedTools],
)]
struct VisitedModule;

#[tokio::test]
async fn a_resource_bound_for_a_visitor_loads_and_answers_by_its_grant() {
    let _serial = SERIAL.lock().await;
    let app = TestApp::builder()
        .module::<VisitedModule>()
        .provide_dyn::<dyn UnitOfWork>(Arc::new(Committing))
        .build()
        .await
        .expect("an opened endpoint boots on its binding");
    LOADS.store(0, Ordering::SeqCst);
    BODIES.store(0, Ordering::SeqCst);

    let session = open_session(app.http(), VISITED_PATH, None).await;
    let body = call_method(
        app.http(),
        VISITED_PATH,
        &session,
        None,
        "resources/read",
        json!({ "uri": format!("note://{FOUND}") }),
    )
    .await;

    let answer = result(&body);
    assert_eq!(
        (LOADS.load(Ordering::SeqCst), BODIES.load(Ordering::SeqCst)),
        (1, 1),
        "{answer}"
    );
    let text = answer["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a text body: {answer}"));
    let note: Value = serde_json::from_str(text).expect("a JSON body");
    assert_eq!(note["title"], "seen", "{note}");
    assert_eq!(
        note["body"],
        Value::Null,
        "the mask strips what the visitor's grant does not read: {note}"
    );
}
