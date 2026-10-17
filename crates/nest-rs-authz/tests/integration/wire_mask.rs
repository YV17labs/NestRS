//! `masked_reply`: a pushed value never went through the `Repo` read whose SQL
//! decides what a relation or a hidden column says, so a rule the rebuilt row
//! cannot decide grants nothing and a denial it cannot decide denies.

use std::sync::Arc;

use nest_rs_authz::{Ability, AbilityBuilder, Action, masked_reply, with_ability};
use serde_json::{Value, json};

use crate::post;

async fn push(ability: Ability, wire: Value) -> Value {
    with_ability(Arc::new(ability), async move {
        masked_reply::<post::Entity>(Action::Read, wire)
    })
    .await
    .expect("the push reconciles with the post model")
}

fn in_org_seven() -> AbilityBuilder {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, post::Entity)
        .when(|p| post::in_org(&p, 7));
    b
}

#[tokio::test]
async fn a_push_whose_grant_reads_a_hidden_column_is_dropped() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, post::Entity)
        .when(|p| p.eq(post::Column::Deleted, false));

    let masked = push(b.build().unwrap(), json!([post::wire(1, 7)])).await;

    assert_eq!(
        masked,
        json!([]),
        "`deleted` never reached the wire, so the grant compared a placeholder: {masked}",
    );
}

#[tokio::test]
async fn a_lone_push_the_ability_refuses_comes_back_null() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, post::Entity)
        .when(|p| p.eq(post::Column::Deleted, false));

    let masked = push(b.build().unwrap(), post::wire(1, 7)).await;

    assert_eq!(masked, Value::Null, "nothing of the row ships: {masked}");
}

#[tokio::test]
async fn a_push_a_relation_grants_is_dropped() {
    let masked = push(
        in_org_seven().build().unwrap(),
        json!([post::wire(1, 7), post::wire(2, 8)]),
    )
    .await;

    assert_eq!(
        masked,
        json!([]),
        "no SQL decided the relation for a pushed row: {masked}"
    );
}

#[tokio::test]
async fn a_relational_denial_denies_a_push() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, post::Entity);
    b.cannot(Action::Read, post::Entity)
        .when(|p| post::in_org(&p, 9));

    let masked = push(b.build().unwrap(), json!([post::wire(1, 7)])).await;

    assert_eq!(masked, json!([]), "{masked}");
}

#[tokio::test]
async fn a_decided_part_of_a_rule_still_admits_a_push() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, post::Entity)
        .when(|p| p.any([p.eq(post::Column::OrgId, 7), post::in_org(&p, 9)]));

    let masked = push(
        b.build().unwrap(),
        json!([post::wire(1, 7), post::wire(2, 8)]),
    )
    .await;

    assert_eq!(
        masked,
        json!([post::wire(1, 7)]),
        "org 7 matches whatever the relation says; org 8 rests on it alone: {masked}",
    );
}

#[tokio::test]
async fn one_warn_names_an_undecidable_push_however_many_rows_it_drops() {
    let logs = nest_rs_testing::LogCapture::install();

    push(
        in_org_seven().build().unwrap(),
        json!([post::wire(1, 7), post::wire(2, 7)]),
    )
    .await;
    push(in_org_seven().build().unwrap(), post::wire(3, 7)).await;

    let event = logs.expect_one(
        nest_rs_authz::TARGET,
        "an ability rule cannot be decided on a pushed value",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("undecidable_on_push")
    );
    assert_eq!(event.field("action").as_deref(), Some("Read"));
    assert!(
        event.field("entity").is_some_and(|e| e.contains("post")),
        "{:?}",
        event.fields,
    );
    assert!(
        !format!("{:?}", event.fields).contains("post 1"),
        "a refusal names no value: {:?}",
        event.fields,
    );
}

/// A pushed row that leaves an exposed column out never sent it: the `None`
/// serde gave the rebuilt model is not a value a rule may be decided on.
#[tokio::test]
async fn a_push_that_leaves_an_exposed_column_out_is_not_judged_on_it() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, post::Entity);
    b.cannot(Action::Read, post::Entity)
        .when(|p| p.is_not_null(post::Column::ArchivedAt));
    let live = json!({ "id": 2, "org_id": 7, "title": "post 2", "archived_at": null });
    let archived = json!({ "id": 3, "org_id": 7, "title": "post 3", "archived_at": 5 });

    let masked = push(
        b.build().unwrap(),
        json!([post::wire(1, 7), live.clone(), archived]),
    )
    .await;

    assert_eq!(
        masked,
        json!([live]),
        "post 1 never said whether it is archived: {masked}",
    );
}

#[tokio::test]
async fn a_push_ships_only_the_keys_it_carried() {
    let mut b = AbilityBuilder::new();
    b.can(Action::Read, post::Entity);

    let masked = push(b.build().unwrap(), post::wire(1, 7)).await;

    assert_eq!(
        masked,
        post::wire(1, 7),
        "no `archived_at: null` the sender never stated: {masked}",
    );
}
