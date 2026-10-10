//! The GraphQL half of `#[expose]`: what the macro *emits* must compile with
//! nothing but `nest-rs-seaorm`'s own `graphql` feature turned on, `Uuid`
//! and `DateTime*` `InputType` impls included.

use nest_rs_seaorm::{Creatable, CrudService, Deletable, Updatable, expose};
use sea_orm::entity::prelude::*;

mod booking {
    use super::*;

    // A `graphql` exposure requires `Serialize`: masking reconstructs from it.
    #[expose(name = "Booking", service = BookingsService, graphql)]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
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
