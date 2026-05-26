//! CASL-style authorization for nestrs.
//!
//! One [`Ability`] definition drives three layers, in request order:
//!
//! 1. **Access gate** — [`Authorize<A, S>`], a poem extractor on a handler
//!    parameter, rejects with `403` unless the actor may perform action `A` on
//!    subject `S` (the class-level check).
//! 2. **Query pre-filter** — [`Ability::condition_for`] lowers the matching
//!    rules' row conditions to a [`sea_orm::Condition`] the data layer applies,
//!    so a query returns only the rows the actor may see.
//! 3. **Response mask** — [`Ability::mask`] / [`Ability::mask_many`] drop
//!    disallowed instances and strip disallowed fields from the response body.
//!
//! Rules are declared once by an [`AbilityFactory`] for the app's actor type;
//! the shared [`Predicate`] representation is what keeps the rows the filter
//! returns and the rows the mask accepts from drifting apart.

mod ability;
mod action;
mod builder;
mod extractor;
mod factory;
mod predicate;
mod shape;
mod subject;

pub use ability::{Ability, FieldSet};
pub use action::{Action, ActionMarker, Create, Delete, Manage, Read, Update};
pub use builder::{AbilityBuilder, RuleSpec};
pub use extractor::Authorize;
pub use factory::AbilityFactory;
pub use predicate::{Predicate, PredicateBuilder};
pub use subject::Subject;

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;

    use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait};

    // A throwaway SeaORM entity so the engine can be exercised without a live
    // database — `condition_for` is rendered to SQL, `can`/`permitted_fields`
    // run against a hand-built `Model`.
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

    fn model(org_id: i32) -> widget::Model {
        widget::Model {
            id: 1,
            org_id,
            name: "ada".into(),
            secret: "hunter2".into(),
        }
    }

    fn ability(org_id: i32, admin: bool) -> Ability {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, org_id));
        if admin {
            b.can(Action::Manage, widget::Entity)
                .when(|p| p.eq(widget::Column::OrgId, org_id));
        }
        b.build()
    }

    #[test]
    fn condition_for_scopes_the_query_to_the_org() {
        let sql = widget::Entity::find()
            .filter(ability(7, false).condition_for::<widget::Entity>(Action::Read))
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(
            sql.contains("org_id"),
            "pre-filter must scope by org_id: {sql}"
        );
    }

    #[test]
    fn no_grant_matches_nothing() {
        // No `can` rule for Delete → the pre-filter must exclude every row.
        let sql = widget::Entity::find()
            .filter(ability(7, false).condition_for::<widget::Entity>(Action::Delete))
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(
            sql.contains("1 = 0"),
            "absent grant must match nothing: {sql}"
        );
    }

    #[test]
    fn can_class_is_the_coarse_gate() {
        let user = ability(7, false);
        assert!(user.can_class(Action::Read, TypeId::of::<widget::Entity>()));
        assert!(!user.can_class(Action::Delete, TypeId::of::<widget::Entity>()));

        // `Manage` is the action wildcard, so an admin passes the gate for any
        // verb on the subject.
        let admin = ability(7, true);
        assert!(admin.can_class(Action::Delete, TypeId::of::<widget::Entity>()));
    }

    #[test]
    fn can_enforces_the_row_condition_in_memory() {
        let user = ability(7, false);
        assert!(user.can::<widget::Entity>(Action::Read, &model(7)));
        assert!(!user.can::<widget::Entity>(Action::Read, &model(8)));
    }

    #[test]
    fn permitted_fields_restricts_to_listed_columns() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 7))
            .fields([widget::Column::Id, widget::Column::Name]);
        let ability = b.build();

        match ability.permitted_fields::<widget::Entity>(Action::Read, &model(7)) {
            FieldSet::Only(cols) => {
                assert!(cols.contains("id"));
                assert!(cols.contains("name"));
                assert!(!cols.contains("secret"), "secret must be masked");
            }
            FieldSet::All => panic!("expected a restricted field set"),
        }
    }

    #[test]
    fn mask_strips_unpermitted_fields_from_the_body() {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity)
            .when(|p| p.eq(widget::Column::OrgId, 7))
            .fields([widget::Column::Id, widget::Column::Name]);
        let ability = b.build();

        let json = ability.mask::<widget::Entity>(Action::Read, &model(7));
        let obj = json.as_object().expect("masked model is a JSON object");
        assert!(obj.contains_key("id"));
        assert!(obj.contains_key("name"));
        assert!(!obj.contains_key("secret"), "secret must be stripped");
        assert!(!obj.contains_key("org_id"), "org_id must be stripped");
    }

    #[test]
    fn mask_many_drops_unauthorized_instances() {
        let user = ability(7, false);
        let rows = [model(7), model(8), model(7)];
        // Only the org-7 rows survive the instance check; with no field
        // restriction every field is kept.
        let masked = user.mask_many::<widget::Entity>(Action::Read, rows.iter());
        assert_eq!(masked.len(), 2);
    }

    #[test]
    fn unrestricted_grant_permits_every_field() {
        // No `.fields(...)` → every field is permitted.
        assert!(matches!(
            ability(7, false).permitted_fields::<widget::Entity>(Action::Read, &model(7)),
            FieldSet::All
        ));
    }
}
