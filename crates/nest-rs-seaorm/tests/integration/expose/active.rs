//! The input conversions `#[expose]` emits (`src/active.rs` in the macro
//! crate): each input names the field carrying each column it sets, by the name
//! serde reads it under and the column's own, renamed or not, so a unique
//! conflict names what the client sent; and every entity registers its table,
//! whose unique keys the binding reads at boot, an input written by hand being
//! able to name them too. Postgres proves the keys in the e2e suite; this proves
//! the declaration.

use std::collections::BTreeSet;

use nest_rs_seaorm::__private::{UniqueKeyRegistration, inventory};
use nest_rs_seaorm::{CreateModel, UpdateModel};

mod account {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Account")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "active_probe_accounts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        pub org_id: i32,
        #[expose(input(create, update))]
        #[sea_orm(column_name = "email_address")]
        pub email: String,
        #[expose(input(create))]
        pub r#type: String,
        #[expose(input(update))]
        pub nickname: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod receipt {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Receipt")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "active_probe_receipts")]
    pub(super) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub total: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[test]
fn each_input_names_the_field_carrying_each_column_it_sets() {
    type Create = account::CreateAccount;
    type Update = account::UpdateAccount;

    assert_eq!(Create::field_for("email_address"), Some("email"));
    assert_eq!(Create::field_for("type"), Some("type"));
    assert_eq!(Update::field_for("email_address"), Some("email"));
    assert_eq!(Update::field_for("nickname"), Some("nickname"));

    for (column, why) in [
        ("email", "the field's name is no column's"),
        ("id", "the key is seeded, never sent"),
        ("org_id", "a column no input carries"),
        ("nickname", "a column the update alone carries"),
    ] {
        assert_eq!(Create::field_for(column), None, "{why}");
    }
    assert_eq!(
        Update::field_for("type"),
        None,
        "the create alone carries it"
    );
}

#[test]
fn every_exposed_entity_registers_its_table_whether_or_not_it_declares_an_input() {
    let registered: BTreeSet<(Option<String>, &str)> = inventory::iter::<UniqueKeyRegistration>
        .into_iter()
        .map(|entry| ((entry.schema)(), (entry.table)()))
        .collect();

    for table in ["active_probe_accounts", "active_probe_receipts"] {
        assert!(registered.contains(&(None, table)), "{registered:?}");
    }
}
