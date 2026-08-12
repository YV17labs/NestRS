//! `src/exposures/relations.rs` — the loader bridges, and the case that used to
//! be refused: **two foreign keys from one child to one parent**.
//!
//! `RelatedTo<Parent>` is keyed on the parent's entity type, so a child
//! declaring two `belongs_to` at the same parent produced two impls of one
//! trait — coherence error `E0119` with a span inside the expansion. The macro
//! refused it at parse time and pointed at a hand-written `#[field_resolver]`.
//!
//! It is now the `Via` type parameter's job: `#[expose]` emits one marker per
//! `belongs_to` beside the child entity, and the parent's `HasMany` names the
//! column — `#[expose(via = "reporter_id")]`. The `SoleForeignKey` default is
//! emitted only while the child points at that parent once, so an ambiguous
//! relation that names nothing is a compile error rather than a silent pick.

use nest_rs_resource::expose;
use nest_rs_seaorm::CrudService;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

pub mod tickets {
    use super::*;

    #[expose(name = "Ticket", service = TicketsService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "tickets",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: String,
        #[expose]
        pub reporter_id: Uuid,
        #[expose]
        pub assignee_id: Uuid,
        // Both point at the same parent. Each gets its own `by_<col>` loader,
        // its own `ByReporterId` / `ByAssigneeId` marker, and its own
        // `RelatedTo<people::Entity, …>` impl — and *neither* gets the
        // `SoleForeignKey` one.
        #[sea_orm(
            belongs_to,
            from = "reporter_id",
            to = "id",
            relation_enum = "Reporter"
        )]
        #[expose]
        pub reporter: HasOne<super::people::Entity>,
        #[sea_orm(
            belongs_to,
            from = "assignee_id",
            to = "id",
            relation_enum = "Assignee"
        )]
        #[expose]
        pub assignee: HasOne<super::people::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}

    pub struct TicketsService;

    impl CrudService for TicketsService {
        type Entity = Entity;
    }
}

pub mod people {
    use super::*;

    #[expose(name = "Person", service = PeopleService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "people",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
        // Two relations, one child entity, told apart by the column each
        // follows. Without `via` neither would compile — which is the point.
        #[sea_orm(has_many, relation_enum = "Reported", via_rel = "Reporter")]
        #[expose(via = "reporter_id")]
        pub reported: HasMany<super::tickets::Entity>,
        #[sea_orm(has_many, relation_enum = "Assigned", via_rel = "Assignee")]
        #[expose(via = "assignee_id")]
        pub assigned: HasMany<super::tickets::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}

    pub struct PeopleService;

    impl CrudService for PeopleService {
        type Entity = Entity;
    }
}

/// The two `via` relations resolve to *different* loaders. Asserted on the
/// projected associated types rather than on a running query: a regression that
/// pointed both fields at one key would still compile and still return rows —
/// the wrong ones.
#[test]
fn two_foreign_keys_to_one_parent_resolve_through_separate_loaders() {
    use nest_rs_resource::RelatedTo;
    use std::any::TypeId;

    type ByReporter = <tickets::Entity as RelatedTo<people::Entity, tickets::ByReporterId>>::Loader;
    type ByAssignee = <tickets::Entity as RelatedTo<people::Entity, tickets::ByAssigneeId>>::Loader;

    assert_ne!(
        TypeId::of::<ByReporter>(),
        TypeId::of::<ByAssignee>(),
        "each foreign key owns its own batched loader",
    );
    assert_eq!(
        TypeId::of::<ByReporter>(),
        TypeId::of::<tickets::TicketsServiceByReporterId>(),
        "the marker resolves to the loader `#[dataloader]` named for that column",
    );
}

/// A child pointing at a parent **once** keeps the default, so every relation
/// written before `via` existed still resolves with nothing to declare.
#[test]
fn a_sole_foreign_key_still_resolves_without_naming_a_column() {
    use nest_rs_resource::{RelatedTo, SoleForeignKey};
    use std::any::TypeId;

    // `Note` points at `Person` once; `RelatedTo<Person>` is `RelatedTo<Person,
    // SoleForeignKey>` by the trait's default type parameter.
    type Default = <notes::Entity as RelatedTo<people::Entity>>::Loader;
    type Explicit = <notes::Entity as RelatedTo<people::Entity, SoleForeignKey>>::Loader;
    assert_eq!(TypeId::of::<Default>(), TypeId::of::<Explicit>());
    assert_eq!(
        TypeId::of::<Default>(),
        TypeId::of::<notes::NotesServiceByAuthorId>(),
    );
}

pub mod notes {
    use super::*;

    #[expose(name = "Note", service = NotesService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "notes",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub body: String,
        #[expose]
        pub author_id: Uuid,
        #[sea_orm(belongs_to, from = "author_id", to = "id")]
        #[expose]
        pub author: HasOne<super::people::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}

    pub struct NotesService;

    impl CrudService for NotesService {
        type Entity = Entity;
    }
}
