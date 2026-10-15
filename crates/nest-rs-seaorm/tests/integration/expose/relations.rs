//! `src/graphql/relations.rs` — the loader bridges, including **two foreign
//! keys from one child to one parent**.

use nest_rs_seaorm::{CrudService, expose};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

pub(super) mod tickets {
    use super::*;

    #[expose(name = "Ticket", service = TicketsService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "tickets",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub title: String,
        #[expose]
        pub reporter_id: Uuid,
        #[expose]
        pub assignee_id: Uuid,
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

    pub(crate) struct TicketsService;

    impl CrudService for TicketsService {
        type Entity = Entity;
    }
}

pub(super) mod people {
    use super::*;

    #[expose(name = "Person", service = PeopleService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "people",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub name: String,
        #[sea_orm(has_many, relation_enum = "Reported", via_rel = "Reporter")]
        #[expose(via = "reporter_id")]
        pub reported: HasMany<super::tickets::Entity>,
        #[sea_orm(has_many, relation_enum = "Assigned", via_rel = "Assignee")]
        #[expose(via = "assignee_id")]
        pub assigned: HasMany<super::tickets::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}

    pub(crate) struct PeopleService;

    impl CrudService for PeopleService {
        type Entity = Entity;
    }
}

/// Asserted on the projected types: a regression pointing both fields at one
/// key would still compile and return rows — the wrong ones.
#[test]
fn two_foreign_keys_to_one_parent_resolve_through_separate_loaders() {
    use nest_rs_seaorm::graphql::RelatedTo;
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

#[test]
fn a_sole_foreign_key_still_resolves_without_naming_a_column() {
    use nest_rs_seaorm::graphql::{RelatedTo, SoleForeignKey};
    use std::any::TypeId;

    type Default = <notes::Entity as RelatedTo<people::Entity>>::Loader;
    type Explicit = <notes::Entity as RelatedTo<people::Entity, SoleForeignKey>>::Loader;
    assert_eq!(TypeId::of::<Default>(), TypeId::of::<Explicit>());
    assert_eq!(
        TypeId::of::<Default>(),
        TypeId::of::<notes::NotesServiceByAuthorId>(),
    );
}

pub(super) mod notes {
    use super::*;

    #[expose(name = "Note", service = NotesService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "notes",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
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

    pub(crate) struct NotesService;

    impl CrudService for NotesService {
        type Entity = Entity;
    }
}

/// A key sea-orm names through heck and through `enum_name`: `ownerID`'s
/// variant is `OwnerId`, the primary key's `Key`. The compile is the assertion
/// — a loader spelling either variant otherwise names a `Column` that does
/// not exist.
#[expect(
    non_snake_case,
    reason = "the fixture: a column whose ident sea-orm does not split on `_`"
)]
pub(super) mod shelves {
    use super::*;

    #[expose(name = "Shelf", service = ShelvesService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "shelves",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false, enum_name = "Key")]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub ownerID: Uuid,
        #[sea_orm(belongs_to, from = "ownerID", to = "id")]
        #[expose]
        pub owner: HasOne<super::people::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}

    pub(crate) struct ShelvesService;

    impl CrudService for ShelvesService {
        type Entity = Entity;
    }
}

#[test]
fn a_key_is_named_by_the_variant_sea_orm_derives() {
    use nest_rs_seaorm::graphql::RelatedTo;
    use std::any::TypeId;

    assert!(matches!(shelves::Column::Key, shelves::Column::Key));
    assert!(matches!(shelves::Column::OwnerId, shelves::Column::OwnerId));
    assert_eq!(
        TypeId::of::<<shelves::Entity as RelatedTo<people::Entity>>::Loader>(),
        TypeId::of::<shelves::ShelvesServiceByOwnerID>(),
    );
}
