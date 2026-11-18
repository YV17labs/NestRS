//! The GraphQL inputs of `#[expose]` (`src/input.rs` in the macro crate): what
//! the macro *emits* must compile with nothing but `nest-rs-seaorm`'s own
//! `graphql` feature turned on, its `DateTime*` `InputType` impls included. The
//! rules an input declares hold at the edge that decodes it, a hand-written
//! create's included, and an id it carries takes a UUID of any version.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Create};
use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::{Json, StatusCode, controller, routes};
use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable, expose};
use nest_rs_testing::TestApp;
use sea_orm::entity::prelude::*;

mod booking {
    use super::*;

    #[expose(name = "Booking", service = BookingsService, graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "bookings")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update))]
        pub guest_id: Uuid,
        #[expose(input(create, update))]
        pub starts_at: Option<DateTimeWithTimeZone>,
        #[expose(input(create, update), validate(length(min = 1)))]
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub(super) struct BookingsService;

    impl CrudService for BookingsService {
        type Entity = Entity;
    }

    impl Creatable for BookingsService {
        type Create = CreateBooking;
    }

    impl Updatable for BookingsService {
        type Update = UpdateBooking;
    }

    impl Deletable for BookingsService {}
}

/// The assertion is the compile above; this pins the shape: an input carries an
/// instant in the entity's own type, and an id as the wire row reads it, built
/// from the column's `Uuid` and read back as one.
#[test]
fn a_graphql_input_carries_an_instant_as_its_column_and_an_id_as_the_wire_reads_it() {
    let guest_id = Uuid::now_v7();
    let create = booking::CreateBooking {
        guest_id: guest_id.into(),
        starts_at: None,
        label: "window seat".into(),
    };
    assert_eq!(Uuid::from(create.guest_id), guest_id);
    assert!(create.starts_at.is_none());

    let wire = booking::Booking::from(&booking::Model {
        id: guest_id,
        guest_id,
        starts_at: None,
        label: "window seat".into(),
    });
    assert_eq!(Uuid::from(wire.id), guest_id);
}

mod ticket {
    use super::*;

    #[expose(name = "Ticket", service = TicketsService, graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "tickets")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create), validate(length(min = 1)))]
        pub label: String,
        #[expose(input(create))]
        pub board_id: Option<Uuid>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    #[injectable]
    #[derive(Default)]
    pub(super) struct TicketsService;

    impl CrudService for TicketsService {
        type Entity = Entity;
    }

    impl Creatable for TicketsService {
        type Create = CreateTicket;
    }
}

/// Grants every caller `Create` on tickets, so the class gate admits it.
#[injectable]
#[derive(Default, Layer)]
struct GrantsCreate;

#[async_trait]
impl Guard for GrantsCreate {
    fn __attaches_ability(&self) -> bool {
        true
    }

    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let mut ability = AbilityBuilder::new();
        ability.can(nest_rs_authz::Action::Create, ticket::Entity);
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            ability
                .build()
                .map_err(|_malformed| Denial::internal("malformed rules"))?,
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for GrantsCreate {}

/// A create written by hand on each edge, over the service's own
/// `Creatable::create` and the bare input `#[expose]` emits.
#[controller(path = "/tickets")]
struct TicketsController {
    #[inject]
    svc: Arc<ticket::TicketsService>,
}

#[routes]
impl TicketsController {
    #[post("/")]
    #[authorize(Create, ticket::Entity)]
    async fn create(
        &self,
        Json(input): Json<ticket::CreateTicket>,
    ) -> Result<Json<ticket::Ticket>, nest_rs_seaorm::ServiceError> {
        let row = Creatable::create(&*self.svc, input).await?;
        Ok(Json(ticket::Ticket::from(&row)))
    }
}

#[resolver]
struct TicketsResolver {
    #[inject]
    svc: Arc<ticket::TicketsService>,
}

#[operations]
impl TicketsResolver {
    #[mutation]
    #[authorize(Create, ticket::Entity)]
    async fn create_ticket(
        &self,
        input: ticket::CreateTicket,
    ) -> Result<ticket::Ticket, nest_rs_seaorm::ServiceError> {
        let row = Creatable::create(&*self.svc, input).await?;
        Ok(ticket::Ticket::from(&row))
    }
}

#[module(
    imports = [GraphqlModule, nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [ticket::TicketsService, GrantsCreate, TicketsController, TicketsResolver],
)]
struct TicketsModule;

#[module(imports = [TicketsModule], providers = [GlobalGuards<(GrantsCreate,)>])]
struct TicketsApp;

/// What the edge admits reaches a pool refusing every acquire, an ordinary
/// failure, never a panic of the driver's.
async fn tickets() -> TestApp {
    crate::over_a_pool_refusing_every_acquire()
        .await
        .module::<TicketsApp>()
        .build()
        .await
        .expect("a hand-written create on each edge boots")
}

#[tokio::test]
async fn a_hand_written_create_taking_the_bare_input_is_validated_over_http() {
    let app = tickets().await;

    let refused = app
        .http()
        .post("/tickets")
        .body_json(&serde_json::json!({ "label": "" }))
        .send()
        .await;

    refused.assert_status(StatusCode::BAD_REQUEST);
    let problem: serde_json::Value = refused.json().await.value().deserialize();
    assert_eq!(problem["errors"]["label"][0]["code"], "length", "{problem}");
}

#[tokio::test]
async fn a_hand_written_create_taking_the_bare_input_is_validated_over_graphql() {
    let app = tickets().await;

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "mutation { createTicket(input: { label: \"\" }) { id } }"
        }))
        .send()
        .await;

    resp.assert_status_is_ok();
    let reply: serde_json::Value = resp.json().await.value().deserialize();
    assert_eq!(
        reply["errors"][0]["message"], "validation failed",
        "{reply}"
    );
    assert_eq!(
        reply["errors"][0]["extensions"]["code"], "INVALID_ARGUMENT",
        "{reply}"
    );
}

/// A key no framework mints: a UUID, version 4, as an identity provider, a table
/// imported from elsewhere or a browser's `crypto.randomUUID()` draws one.
const V4: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";

/// A create whose input relates the row to a key of any UUID version passes the
/// edge and reaches the service, whose pool has no server: the opaque `INTERNAL`,
/// never the edge's refusal.
#[tokio::test]
async fn a_create_relates_a_row_to_a_key_of_any_uuid_version_over_http() {
    let app = tickets().await;

    let reached = app
        .http()
        .post("/tickets")
        .body_json(&serde_json::json!({ "label": "aisle", "board_id": V4 }))
        .send()
        .await;

    reached.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    let problem: serde_json::Value = reached.json().await.value().deserialize();
    assert_eq!(problem["code"], "INTERNAL", "{problem}");
}

#[tokio::test]
async fn a_create_relates_a_row_to_a_key_of_any_uuid_version_over_graphql() {
    let app = tickets().await;

    let reply: serde_json::Value = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": format!(
                "mutation {{ createTicket(input: {{ label: \"aisle\", boardId: \"{V4}\" }}) {{ id }} }}"
            )
        }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize();

    assert_eq!(
        reply["errors"][0]["extensions"]["code"], "INTERNAL",
        "{reply}"
    );
}

/// A ledger an app holds: its own key the framework minted, its account's key a
/// value another system drew, which a replacement carries back as it reads it.
mod ledger {
    use super::*;

    #[expose(name = "Ledger", graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "held_ledgers")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update))]
        pub account_id: Uuid,
        #[expose(input(create, update))]
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    #[injectable]
    #[derive(Default)]
    pub(super) struct LedgersService;

    impl CrudService for LedgersService {
        type Entity = Entity;
    }

    impl Creatable for LedgersService {
        type Create = CreateLedger;
    }

    impl Updatable for LedgersService {
        type Update = UpdateLedger;
    }

    /// The row as the app holds it.
    pub(super) const ROW: &str = "01920000-0000-7000-8000-0000000001a1";
}

#[injectable]
#[derive(Default)]
struct WritesLedgers;

impl nest_rs_authz::AbilityFactory for WritesLedgers {
    type Actor = crate::Anyone;

    fn define(&self, _actor: &crate::Anyone, ab: &mut AbilityBuilder) {
        ab.can(nest_rs_authz::Action::Read, ledger::Entity);
        ab.can(nest_rs_authz::Action::Update, ledger::Entity);
    }
}

#[controller(path = "/ledgers")]
#[use_guards(
    crate::always_authenticated::AlwaysAuthenticated,
    nest_rs_authz::AbilityGuard<WritesLedgers>
)]
struct LedgersController {
    #[inject]
    svc: Arc<ledger::LedgersService>,
}

#[nest_rs_http::crud(service = ledger::LedgersService, ops = [update])]
impl LedgersController {}

#[resolver]
struct LedgersResolver {
    #[inject]
    svc: Arc<ledger::LedgersService>,
}

#[nest_rs_graphql::crud(service = ledger::LedgersService, ops = [update])]
impl LedgersResolver {}

/// The row the app holds, as a read of it answers.
#[resolver]
struct HeldLedgerResolver;

#[operations]
impl HeldLedgerResolver {
    #[query]
    #[authorize(nest_rs_authz::Read, ledger::Entity)]
    async fn held_ledger(&self) -> Result<ledger::Ledger, nest_rs_seaorm::ServiceError> {
        Ok(ledger::Ledger::from(&ledger::Model {
            id: ledger::ROW.parse().expect("a UUID"),
            account_id: V4.parse().expect("a UUID"),
            label: "rent".to_owned(),
        }))
    }
}

#[module(
    imports = [
        nest_rs_seaorm::SeaOrmDatabaseModule,
        GraphqlModule::for_root(nest_rs_graphql::GraphqlConfig::default()),
    ],
    providers = [
        ledger::LedgersService,
        WritesLedgers,
        nest_rs_authz::AbilityGuard<WritesLedgers>,
        crate::always_authenticated::AlwaysAuthenticated,
        nest_rs_http::EndpointGuards<
            nest_rs_graphql::GraphqlView,
            (
                crate::always_authenticated::AlwaysAuthenticated,
                nest_rs_authz::AbilityGuard<WritesLedgers>,
            ),
        >,
        LedgersController,
        LedgersResolver,
        HeldLedgerResolver,
    ],
)]
struct LedgersModule;

/// An app whose every statement reaches a pool refusing every acquire: what the
/// edge admits answers the opaque `500`.
async fn ledgers() -> TestApp {
    crate::over_a_pool_refusing_every_acquire()
        .await
        .module::<LedgersModule>()
        .build()
        .await
        .expect("the app boots: the server's absence is a runtime fact")
}

/// A replacement carrying back the account key the row holds, as it reads it,
/// passes the edge whatever the key's UUID version, and reaches the service.
#[tokio::test]
async fn an_update_carries_back_a_key_of_any_uuid_version_over_http() {
    let app = ledgers().await;

    let reached = app
        .http()
        .put(format!("/ledgers/{}", ledger::ROW))
        .body_json(&serde_json::json!({ "account_id": V4, "label": "renamed" }))
        .send()
        .await;

    reached.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    let problem: serde_json::Value = reached.json().await.value().deserialize();
    assert_eq!(problem["code"], "INTERNAL", "{problem}");
}

#[tokio::test]
async fn an_update_carries_back_a_key_of_any_uuid_version_over_graphql() {
    let app = ledgers().await;
    let ask = |body: serde_json::Value| {
        let app = &app;
        async move {
            app.http()
                .post("/graphql")
                .body_json(&body)
                .send()
                .await
                .json()
                .await
                .value()
                .deserialize::<serde_json::Value>()
        }
    };

    let held = ask(serde_json::json!({ "query": "{ heldLedger { id accountId } }" })).await;
    let row = &held["data"]["heldLedger"];
    assert_eq!(row["accountId"], V4, "{held}");

    let replaced = ask(serde_json::json!({
        "query": "mutation($id: ID!, $accountId: ID!) { updateLedger(id: $id, input: { accountId: $accountId, label: \"renamed\" }) { id } }",
        "variables": { "id": row["id"], "accountId": row["accountId"] },
    }))
    .await;
    assert_eq!(
        replaced["errors"][0]["extensions"]["code"], "INTERNAL",
        "{replaced}"
    );
}

/// An input's id that is no UUID at all is refused where the edge decodes it,
/// quoting nothing it was sent.
#[tokio::test]
async fn an_id_an_input_carries_that_is_no_uuid_is_refused_quoting_nothing() {
    const NO_UUID: &str = "board-SECRET-MARK";
    let app = tickets().await;

    let refused = app
        .http()
        .post("/tickets")
        .body_json(&serde_json::json!({ "label": "aisle", "board_id": NO_UUID }))
        .send()
        .await;
    refused.assert_status(StatusCode::BAD_REQUEST);
    let problem: serde_json::Value = refused.json().await.value().deserialize();
    assert_eq!(problem["code"], "INVALID_ARGUMENT", "{problem}");
    assert!(
        !problem.to_string().contains(NO_UUID),
        "quotes the value: {problem}"
    );

    let reply: serde_json::Value = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": format!(
                "mutation {{ createTicket(input: {{ label: \"aisle\", boardId: \"{NO_UUID}\" }}) {{ id }} }}"
            )
        }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize();
    assert!(reply["errors"][0].is_object(), "refused: {reply}");
    assert!(reply["data"]["createTicket"].is_null(), "{reply}");
    assert!(
        !reply.to_string().contains(NO_UUID),
        "quotes the value: {reply}"
    );
}

/// The id an input carries is GraphQL's `ID`, as a reply and a `#[crud]` id
/// argument read it.
#[tokio::test]
async fn an_id_an_input_carries_is_graphql_s_id() {
    let app = tickets().await;

    let reply: serde_json::Value = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": r#"{ __type(name: "CreateTicket") { inputFields { name type { name } } } }"#
        }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize();

    let fields = reply["data"]["__type"]["inputFields"]
        .as_array()
        .expect("the input's fields");
    let board = fields
        .iter()
        .find(|field| field["name"] == "boardId")
        .expect("the id field");
    assert_eq!(board["type"]["name"], "ID", "{reply}");
}

mod enum_column {
    use nest_rs_core::wire_enum;
    use nest_rs_seaorm::__private::async_graphql;
    use nest_rs_seaorm::{CrudService, expose};
    use sea_orm::entity::prelude::*;

    #[wire_enum(graphql)]
    #[derive(EnumIter, DeriveActiveEnum)]
    #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
    #[serde(rename_all = "lowercase")]
    pub enum Stage {
        #[sea_orm(string_value = "draft")]
        Draft,
        #[sea_orm(string_value = "shipped")]
        Shipped,
    }

    #[expose(name = "Release", service = ReleasesService, graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "releases")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update))]
        pub stage: Stage,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub(super) struct ReleasesService;

    impl CrudService for ReleasesService {
        type Entity = Entity;
    }

    #[test]
    fn a_graphql_wire_enum_is_a_schema_enum_on_both_sides_of_an_operation() {
        assert_eq!(
            <Stage as async_graphql::InputType>::type_name(),
            "Stage",
            "the enum names itself in the SDL",
        );
        assert_eq!(
            <Stage as async_graphql::OutputType>::type_name(),
            <Stage as async_graphql::InputType>::type_name(),
        );
    }

    #[test]
    fn the_enum_column_reaches_the_generated_input_too() {
        let create = CreateRelease {
            stage: Stage::Draft,
        };
        assert_eq!(create.stage, Stage::Draft);
    }
}

mod account {
    use super::*;

    #[expose(name = "Account")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "accounts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[sea_orm(column_name = "display_name")]
        #[expose(input(update))]
        pub name: String,
        #[expose(input(update))]
        pub bio: Option<String>,
        pub note: String,
        #[expose]
        pub plan: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// What a client may carry back as it stands is what the input carries, which
/// `#[expose]` shows on the wire too, by the column's own name.
#[test]
fn an_update_reads_back_only_what_a_reply_shows() {
    use nest_rs_seaorm::UpdateModel;

    type Update = account::UpdateAccount;

    assert!(<Update as UpdateModel<account::Entity>>::reads_back(
        "display_name"
    ));
    assert!(<Update as UpdateModel<account::Entity>>::reads_back("bio"));
    assert!(!<Update as UpdateModel<account::Entity>>::reads_back(
        "note"
    ));
    assert!(
        !<Update as UpdateModel<account::Entity>>::reads_back("plan"),
        "shown, but the input does not carry it",
    );
    assert!(
        !<Update as UpdateModel<account::Entity>>::reads_back("name"),
        "the wire's name is not the column's",
    );
}
