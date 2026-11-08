//! The GraphQL inputs of `#[expose]` (`src/input.rs` in the macro crate): what
//! the macro *emits* must compile with nothing but `nest-rs-seaorm`'s own
//! `graphql` feature turned on, `Uuid` and `DateTime*` `InputType` impls
//! included. The rules an input declares hold at the edge that decodes it, a
//! hand-written create's included.

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

    // A `graphql` exposure requires `Serialize`: masking reconstructs from it.
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

/// The assertion is the compile above; this pins the shape so the fixture
/// keeps a `Uuid` and a timestamp reaching the GraphQL input, unconverted.
#[test]
fn a_graphql_input_carries_the_entitys_own_uuid_and_timestamp_types() {
    let guest_id = Uuid::now_v7();
    let create = booking::CreateBooking {
        guest_id,
        starts_at: None,
        label: "window seat".into(),
    };
    assert_eq!(create.guest_id, guest_id);
    assert!(create.starts_at.is_none());

    let wire = booking::Booking::from(&booking::Model {
        id: guest_id,
        guest_id,
        starts_at: None,
        label: "window seat".into(),
    });
    assert_eq!(wire.id, guest_id.to_string());
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

async fn tickets() -> TestApp {
    crate::over_an_unconnected_pool()
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
