//! The GraphQL half of `#[expose]`: what the macro *emits* must compile with
//! nothing but `nest-rs-resource`'s own `graphql` feature turned on.
//!
//! The class this closes: `#[expose(input(...))]` re-emits the column's own
//! type into the generated `InputObject`, and an entity's columns are `Uuid`
//! and `DateTime*` by construction — so the derive needs those scalars'
//! `InputType` impls, which `async-graphql` gates behind features. Left to the
//! consumer, the only signal was `the trait bound uuid::Uuid: InputType is not
//! satisfied` pointing at a foreign-key field they never chose a type for.
//! `nest-rs-resource` declares them now, because it is the crate whose macro
//! creates the requirement.

use nest_rs_resource::expose;
use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable};
use sea_orm::entity::prelude::*;

mod booking {
    use super::*;

    // `Serialize` is what response masking reconstructs from, so a `graphql`
    // exposure requires it where the wire-only fixtures above do not.
    #[expose(name = "Booking", service = BookingsService, graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "bookings")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        // A foreign key written by the client: the shape every relation a
        // developer would want to create goes through.
        #[expose(input(create, update))]
        pub guest_id: Uuid,
        #[expose(input(create, update))]
        pub starts_at: Option<DateTimeWithTimeZone>,
        #[expose(input(create, update), validate(length(min = 1)))]
        pub label: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    pub struct BookingsService;

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

/// The assertion is the compile above; this pins the shape so the fixture can't
/// be reduced to something that no longer exercises it — a `Uuid` and a
/// timestamp reaching the generated GraphQL input, unconverted.
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

    // The output DTO renders a `Uuid` as a string, so the two halves genuinely
    // differ — this is not the same impl arriving twice.
    let wire = booking::Booking::from(&booking::Model {
        id: guest_id,
        guest_id,
        starts_at: None,
        label: "window seat".into(),
    });
    assert_eq!(wire.id, guest_id.to_string());
}
