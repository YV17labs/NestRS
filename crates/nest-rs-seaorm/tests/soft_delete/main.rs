//! The boot audit refusing a `soft_delete` entity no service tombstones. Its own
//! binary: the half-wired entity sits in the link-time registry every app audits.

use nest_rs_resource::expose;
use nest_rs_seaorm::audit_soft_delete_bindings;
use nest_rs_seaorm::sea_orm::entity::prelude::*;
use nest_rs_seaorm::{CrudService, Deletable};

/// Both halves declared — the shape `nestrs g resource` scaffolds.
mod bound {
    use super::*;

    // `timestamps` emits the `ActiveModelBehavior` impl `CrudService` requires.
    #[expose(name = "BoundRow", service = RowsService, soft_delete, timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "audit_bound_row")]
    pub(super) struct Model {
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
    pub(super) struct RowsService;

    impl CrudService for RowsService {
        type Entity = Entity;

        fn soft_delete_column() -> Option<Column> {
            Some(Column::DeletedAt)
        }
    }

    impl Deletable for RowsService {}
}

/// The entity keeps its flag, the service lost its override: `DELETE` destroys
/// the row.
mod unbound {
    use super::*;

    #[expose(name = "UnboundRow", service = OrphansService, soft_delete, timestamps)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(table_name = "audit_unbound_row")]
    pub(super) struct Model {
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
    pub(super) struct OrphansService;

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
    let text = audit_soft_delete_bindings()
        .expect_err("the unbound entity is still linked into this binary")
        .to_string();
    assert!(
        !text.contains("audit_bound_row"),
        "an entity whose service overrides the column is not a mismatch: {text}",
    );
}
