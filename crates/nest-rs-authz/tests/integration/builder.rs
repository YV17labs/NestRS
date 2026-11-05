//! Builder-level invariants exercised through the crate's public API: denial
//! overrides grant, multiple grants compose into an OR'd pre-filter, `fields()`
//! narrows masking, and a `RuleSpec` commits even if its terminal call is
//! omitted (the `Drop` impl is the commit point).

use std::any::TypeId;

use nest_rs_authz::{AbilityBuilder, Action, FieldSet};
use nest_rs_guards::Scope;
use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait};

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub org_id: i32,
        pub name: String,
        pub secret: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

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

use crate::{child, parent};

#[test]
fn denial_overrides_a_matching_grant() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::OrgId, 7));
    b.cannot(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::Id, 13));
    let ability = b.build().expect("valid test ability");

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
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::OrgId, 7));
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::Id, 99));
    let ability = b.build().expect("valid test ability");

    assert!(ability.can::<widget::Entity>(Action::Read, &model(1, 7)));
    assert!(ability.can::<widget::Entity>(Action::Read, &model(99, 999)));
}

#[test]
fn fields_narrow_to_the_listed_columns() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::OrgId, 7))
        .fields([widget::Column::Id, widget::Column::Name]);
    let ability = b.build().expect("valid test ability");

    match ability.permitted_fields::<widget::Entity>(Action::Read, &model(1, 7)) {
        FieldSet::Only(cols) => {
            assert!(cols.contains("id"));
            assert!(cols.contains("name"));
            assert!(!cols.contains("secret"));
            assert!(!cols.contains("org_id"));
        }
        _ => panic!("expected a restricted field set"),
    }
}

#[test]
fn manage_grants_pass_the_class_gate_for_every_action() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, widget::Entity);
    let ability = b.build().expect("valid test ability");

    let subj = TypeId::of::<widget::Entity>();
    for action in [Action::Read, Action::Create, Action::Update, Action::Delete] {
        assert!(
            ability.can_class(action, subj),
            "Manage must satisfy the gate for {action:?}",
        );
    }
}

#[test]
fn a_rule_spec_commits_when_its_statement_ends_without_a_terminal_call() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, widget::Entity);
    let ability = b.build().expect("valid test ability");

    assert!(ability.can::<widget::Entity>(Action::Read, &model(1, 7)));
}

#[test]
fn a_malformed_cannot_fails_ability_construction() {
    // Declares `child` as the related entity while `Parent` points at `parent`.
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, child::Entity);
    b.cannot(Action::Read, child::Entity).when(|p| {
        p.related::<child::Entity, _>(child::Relation::Parent, |c| c.eq(child::Column::Id, 1))
    });
    // `Ability` is not `Debug`, so `expect_err` is unavailable.
    let err = match b.build() {
        Ok(_) => panic!("a malformed cannot must fail construction"),
        Err(e) => e,
    };

    assert_eq!(err.kind, "denial");
    assert!(
        err.subject.contains("child"),
        "the error names the subject entity: {}",
        err.subject
    );
    let msg = err.to_string();
    assert!(
        msg.contains("relation predicate"),
        "the error explains the cause: {msg}"
    );
}

#[test]
fn a_malformed_grant_also_fails_construction() {
    let logs = nest_rs_testing::LogCapture::install();
    let mut b = AbilityBuilder::new();
    // `widget` differs from both the subject (`child`) and the relation's target
    // (`parent`), so the `related` field cannot pass by naming either.
    b.can(Action::Read, child::Entity).when(|p| {
        p.related::<widget::Entity, _>(child::Relation::Parent, |w| w.eq(widget::Column::Id, 1))
    });
    let err = match b.build() {
        Ok(_) => panic!("a malformed grant must fail construction"),
        Err(e) => e,
    };
    assert_eq!(err.kind, "grant");

    let event = logs.expect_one(
        "nest_rs::authz",
        "invalid ability relation predicate — denying all rows",
    );
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("relation_table_mismatch"),
        "the two malformations — a relation pointing elsewhere, and a composite \
         key — are told apart by this field, got {:?}",
        event.fields,
    );
    let related = event.field("related").unwrap_or_default();
    assert!(
        related.contains("widget") && !related.contains("child"),
        "…and it names the entity the rule *declared*, not the one it is about: \
         {related:?}",
    );
}

#[test]
fn a_well_formed_relation_predicate_says_nothing() {
    let logs = nest_rs_testing::LogCapture::install();
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, child::Entity).when(|p| {
        p.related::<parent::Entity, _>(child::Relation::Parent, |c| c.eq(parent::Column::Id, 1))
    });
    b.build()
        .unwrap_or_else(|e| panic!("a well-formed relational grant builds: {e}"));
    logs.expect_none(
        "nest_rs::authz",
        "invalid ability relation predicate — denying all rows",
    );
}

#[test]
fn no_grant_for_a_subject_produces_a_one_equals_zero_filter() {
    let ability = AbilityBuilder::new().build().expect("valid test ability");
    let sql = widget::Entity::find()
        .filter(ability.condition_for::<widget::Entity>(Action::Read))
        .build(DatabaseBackend::Postgres)
        .to_string();
    assert!(
        sql.contains("1 = 0"),
        "absent grant ⇒ matches nothing: {sql}"
    );
}

/// May export the org's reports: a capability no table holds.
#[nest_rs_authz::subject]
struct Exporting;

#[test]
fn a_subject_no_table_holds_is_granted_whole_by_class() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, Exporting);
    let ability = b.build().expect("valid test ability");

    assert!(ability.can_class(Action::Manage, TypeId::of::<Exporting>()));
    assert!(
        ability.can_class(Action::Read, TypeId::of::<Exporting>()),
        "`Manage` is the wildcard on a subject no table holds as on an entity",
    );
    assert!(
        !ability.can_class(Action::Read, TypeId::of::<widget::Entity>()),
        "a grant on the capability reaches no entity",
    );
}

#[test]
fn four_grants_on_a_subject_no_table_holds_pass_a_site_gated_on_manage() {
    let mut b = AbilityBuilder::new();
    for action in [Action::Read, Action::Create, Action::Update] {
        b.can(action, Exporting);
    }
    let three = b.build().expect("valid test ability");
    assert!(!three.can_class(Action::Manage, TypeId::of::<Exporting>()));

    let mut b = AbilityBuilder::new();
    for action in [Action::Read, Action::Create, Action::Update, Action::Delete] {
        b.can(action, Exporting);
    }
    let four = b.build().expect("valid test ability");
    assert!(four.can_class(Action::Manage, TypeId::of::<Exporting>()));
}

#[test]
fn a_capability_a_token_was_not_granted_names_the_scope_it_lacks() {
    let narrow: std::sync::Arc<[String]> = std::sync::Arc::from(["reports:read".to_owned()]);
    let mut b = AbilityBuilder::new().with_granted_scopes(Some(narrow));
    b.can(Action::Manage, Exporting)
        .requires_scope(Scope::new("reports:export"));
    let ability = b.build().expect("valid test ability");

    let exporting = TypeId::of::<Exporting>();
    assert!(!ability.can_class(Action::Manage, exporting));
    assert_eq!(
        ability.missing_scopes(Action::Manage, exporting),
        ["reports:export"],
        "the refusal names what a wider token would carry",
    );
}

#[test]
fn a_capability_beside_an_entity_leaves_the_entitys_rows_unwidened() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Manage, Exporting);
    b.can(Action::Read, widget::Entity)
        .when(|p| p.eq(widget::Column::OrgId, 7));
    let ability = b.build().expect("valid test ability");

    assert!(ability.can::<widget::Entity>(Action::Read, &model(1, 7)));
    assert!(!ability.can::<widget::Entity>(Action::Read, &model(1, 8)));
}
