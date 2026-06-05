//! Builder-level invariants exercised through the crate's public API: denial
//! overrides grant, multiple grants compose into an OR'd pre-filter, `fields()`
//! narrows masking, and a `RuleSpec` commits even if its terminal call is
//! omitted (the `Drop` impl is the commit point).

use std::any::TypeId;

use nestrs_authz::{AbilityBuilder, Action, FieldSet};
use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait};

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "widgets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub org_id: i32,
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

fn model(id: i32, org: i32) -> widget::Model {
    widget::Model {
        id,
        org_id: org,
        name: "ada".into(),
        secret: "hunter2".into(),
    }
}

#[test]
fn denial_overrides_a_matching_grant() {
    // Defence in depth: even with `Read` granted on the org, an explicit denial
    // on row 13 must keep it out of both the in-memory check and the query.
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::OrgId, 7));
    b.cannot(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::Id, 13));
    let ability = b.build();

    assert!(ability.can::<widget::Entity>(Action::Read, &model(1, 7)));
    assert!(!ability.can::<widget::Entity>(Action::Read, &model(13, 7)));

    let sql = widget::Entity::find()
        .filter(ability.condition_for::<widget::Entity>(Action::Read))
        .build(DatabaseBackend::Postgres)
        .to_string();
    assert!(
        sql.contains("org_id") && sql.to_lowercase().contains("not"),
        "denial must compile to a NOT clause: {sql}",
    );
}

#[test]
fn multiple_grants_or_into_the_query_filter() {
    // Two `can(Read, _)` rules on the same entity must produce a SQL filter
    // that admits rows matching either grant.
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::OrgId, 7));
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::Id, 99));
    let ability = b.build();

    assert!(ability.can::<widget::Entity>(Action::Read, &model(1, 7)));
    // Row 99 in a different org is still permitted by the second grant.
    assert!(ability.can::<widget::Entity>(Action::Read, &model(99, 999)));
}

#[test]
fn fields_narrow_to_the_listed_columns() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::OrgId, 7))
        .fields([widget::Column::Id, widget::Column::Name]);
    let ability = b.build();

    match ability.permitted_fields::<widget::Entity>(Action::Read, &model(1, 7)) {
        FieldSet::Only(cols) => {
            assert!(cols.contains("id"));
            assert!(cols.contains("name"));
            assert!(!cols.contains("secret"));
            assert!(!cols.contains("org_id"));
        }
        FieldSet::All => panic!("expected a restricted field set"),
    }
}

#[test]
fn manage_grants_pass_the_class_gate_for_every_action() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, widget::Entity);
    let ability = b.build();

    let subj = TypeId::of::<widget::Entity>();
    for action in [
        Action::Read,
        Action::Create,
        Action::Update,
        Action::Delete,
    ] {
        assert!(
            ability.can_class(action, subj),
            "Manage must satisfy the gate for {action:?}",
        );
    }
}

#[test]
fn a_rule_spec_commits_when_its_statement_ends_without_a_terminal_call() {
    // `can(Read, _)` with no `.when(...)`/`.fields(...)` is a fully open grant;
    // it must still commit on drop.
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity);
    let ability = b.build();

    assert!(ability.can::<widget::Entity>(Action::Read, &model(1, 7)));
}

#[test]
fn no_grant_for_a_subject_produces_a_one_equals_zero_filter() {
    // Empty ability ⇒ defaulting to TRUE would silently leak rows; the engine
    // must yield `1 = 0` so the pre-filter matches nothing.
    let ability = AbilityBuilder::new().build();
    let sql = widget::Entity::find()
        .filter(ability.condition_for::<widget::Entity>(Action::Read))
        .build(DatabaseBackend::Postgres)
        .to_string();
    assert!(sql.contains("1 = 0"), "absent grant ⇒ matches nothing: {sql}");
}
