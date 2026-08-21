//! The boot audit that closes the half-wired tombstone.
//!
//! `#[expose(..., soft_delete)]` makes the column addressable and implements
//! `SoftDeletable`; only `CrudService::soft_delete_column` makes `DELETE`
//! tombstone. Dropping the second half leaves an entity that answers `204` and
//! *destroys the row* — the same response a successful tombstone returns, with
//! no warning at boot and none at the delete.
//!
//! The unit tests beside `audit` cover the message. What only a real expansion
//! can prove is that `#[expose]` submits the pair at all, and that the audit
//! reads it back: both entities below are compiled by the decorator, and the
//! verdict is read out of the link-time registry exactly as `SeaOrmDatabaseModule`
//! reads it at boot.

use nest_rs_resource::expose;
use nest_rs_seaorm::audit_soft_delete_bindings;
use nest_rs_seaorm::sea_orm::entity::prelude::*;
use nest_rs_seaorm::{CrudService, Deletable};

/// Both halves declared — the shape `nestrs g resource` scaffolds.
mod bound {
    use super::*;

    // `timestamps` is not decoration: it emits the `ActiveModelBehavior` impl
    // `CrudService`'s own where-clause requires, so the service cannot be
    // written without it.
    #[expose(name = "BoundRow", service = RowsService, soft_delete, timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "audit_bound_row")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update))]
        pub name: String,
        #[expose]
        pub created_at: DateTimeWithTimeZone,
        #[expose]
        pub updated_at: DateTimeWithTimeZone,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }

    #[derive(Default)]
    pub struct RowsService;

    impl CrudService for RowsService {
        type Entity = Entity;

        fn soft_delete_column() -> Option<Column> {
            Some(Column::DeletedAt)
        }
    }

    impl Deletable for RowsService {}
}

/// The trap: the entity keeps its flag, the service lost its override. This is
/// what a refactor — or a service written by hand against `/database/crud/` —
/// produces, and it is irreversible the first time someone calls `DELETE`.
mod unbound {
    use super::*;

    #[expose(name = "UnboundRow", service = OrphansService, soft_delete, timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "audit_unbound_row")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose(input(create, update))]
        pub name: String,
        #[expose]
        pub created_at: DateTimeWithTimeZone,
        #[expose]
        pub updated_at: DateTimeWithTimeZone,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }

    #[derive(Default)]
    pub struct OrphansService;

    impl CrudService for OrphansService {
        type Entity = Entity;
    }

    impl Deletable for OrphansService {}
}

#[test]
fn a_tombstone_column_no_service_writes_refuses_boot() {
    let err = audit_soft_delete_bindings()
        .expect_err("`unbound` declares soft_delete on the entity and nowhere else");
    let text = err.to_string();
    assert!(
        text.contains("audit_unbound_row"),
        "the refusal names the table whose rows would be destroyed: {text}",
    );
    assert!(
        text.contains("OrphansService"),
        "and the service to edit: {text}",
    );
}

#[test]
fn a_correctly_wired_entity_is_not_reported() {
    // Same registry, same walk: the audit must accuse only the half-wired pair.
    // Without this, "refuse when anything is registered" would pass the test
    // above and break every app that uses soft delete correctly.
    let text = audit_soft_delete_bindings()
        .expect_err("the unbound entity is still linked into this binary")
        .to_string();
    assert!(
        !text.contains("audit_bound_row"),
        "an entity whose service overrides the column is not a mismatch: {text}",
    );
}
