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

    type ByReporter =
        <tickets::Entity as RelatedTo<people::Entity, tickets::__ByReporterId>>::Loader;
    type ByAssignee =
        <tickets::Entity as RelatedTo<people::Entity, tickets::__ByAssigneeId>>::Loader;

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

/// A child's page answers the wire type its own `#[expose]` names: the
/// relation reads it off `Exposed`, never off a second association.
#[test]
fn a_child_of_a_relation_answers_the_wire_type_its_own_expose_names() {
    use nest_rs_seaorm::Exposed;
    use nest_rs_seaorm::graphql::RelatedTo;
    use std::any::TypeId;

    fn wire_of<Child, Parent, Via>() -> TypeId
    where
        Child: RelatedTo<Parent, Via>,
    {
        TypeId::of::<<Child as Exposed>::Wire>()
    }

    assert_eq!(
        wire_of::<tickets::Entity, people::Entity, tickets::__ByReporterId>(),
        TypeId::of::<tickets::Ticket>(),
    );
    assert_eq!(
        wire_of::<notes::Entity, people::Entity, nest_rs_seaorm::graphql::SoleForeignKey>(),
        TypeId::of::<notes::Note>(),
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

pub(super) mod racks {
    use super::*;

    #[expose(name = "Rack", service = RacksService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "racks",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[sea_orm(has_many)]
        #[expose]
        pub bins: HasMany<super::bins::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}

    #[nest_rs_core::injectable]
    pub(crate) struct RacksService;

    impl CrudService for RacksService {
        type Entity = Entity;
    }
}

pub(super) mod bins {
    use super::*;

    #[expose(name = "Bin", service = BinsService, graphql)]
    #[sea_orm::model]
    #[derive(Clone, Debug, DeriveEntityModel)]
    #[sea_orm(
        table_name = "bins",
        model_attrs(derive(PartialEq, Serialize, Deserialize))
    )]
    pub(crate) struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
        #[expose]
        pub rack_id: Uuid,
        #[sea_orm(belongs_to, from = "rack_id", to = "id")]
        #[expose]
        pub rack: HasOne<super::racks::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}

    #[nest_rs_core::injectable]
    pub(crate) struct BinsService;

    impl CrudService for BinsService {
        type Entity = Entity;
    }
}

#[nest_rs_core::module(providers = [racks::RacksService])]
struct RacksModule;

#[nest_rs_core::module(providers = [bins::BinsService])]
struct BinsModule;

/// Serves `Bin`, whose `rack` reads a loader `RacksService` owns.
#[nest_rs_graphql::resolver]
struct BinsResolver;

#[nest_rs_graphql::operations]
impl BinsResolver {
    #[query]
    #[public]
    async fn bin(&self) -> Option<bins::Bin> {
        None
    }
}

#[nest_rs_core::module(
    imports = [
        nest_rs_graphql::GraphqlModule,
        nest_rs_seaorm::SeaOrmDatabaseModule,
        BinsModule,
    ],
    providers = [BinsResolver],
)]
struct BinsGraphqlModule;

#[tokio::test]
async fn a_served_relation_whose_loader_owner_is_not_imported_refuses_the_boot_naming_it() {
    let refused = crate::over_an_unconnected_pool()
        .module::<BinsGraphqlModule>()
        .build()
        .await
        .err()
        .expect("every query selecting `rack` would fail");
    let refused = format!("{refused:#}");
    for named in [
        "`Bin.rack`",
        "bins::Entity",
        "RacksService",
        "import `RacksModule`",
    ] {
        assert!(refused.contains(named), "names {named}: {refused}");
    }
}

#[nest_rs_core::module(imports = [BinsGraphqlModule, RacksModule])]
struct ShelvedBinsModule;

#[tokio::test]
async fn a_served_relation_whose_loader_owner_is_imported_boots() {
    crate::over_an_unconnected_pool()
        .module::<ShelvedBinsModule>()
        .build()
        .await
        .expect("the loader `rack` reads is reached");
}

/// Serves `Rack`, whose `bins` reads the loader `BinsService` owns through the
/// child's foreign key — never one of the parent's.
#[nest_rs_graphql::resolver]
struct RacksResolver;

#[nest_rs_graphql::operations]
impl RacksResolver {
    #[query]
    #[public]
    async fn rack(&self) -> Option<racks::Rack> {
        None
    }
}

#[nest_rs_core::module(
    imports = [
        nest_rs_graphql::GraphqlModule,
        nest_rs_seaorm::SeaOrmDatabaseModule,
        RacksModule,
    ],
    providers = [RacksResolver],
)]
struct RacksGraphqlModule;

#[tokio::test]
async fn a_served_has_many_whose_child_service_is_not_imported_refuses_the_boot_naming_it() {
    let refused = crate::over_an_unconnected_pool()
        .module::<RacksGraphqlModule>()
        .build()
        .await
        .err()
        .expect("every query selecting `bins` would fail");
    let refused = format!("{refused:#}");
    for named in [
        "`Rack.bins`",
        "racks::Entity",
        "BinsService",
        "import `BinsModule`",
    ] {
        assert!(refused.contains(named), "names {named}: {refused}");
    }
    assert!(
        !refused.contains("`Bin.rack`"),
        "the child's own relation reads the parent's loader, which is reached: {refused}"
    );
}

#[nest_rs_core::module(imports = [RacksGraphqlModule, BinsModule])]
struct BinnedRacksModule;

#[tokio::test]
async fn a_served_has_many_whose_child_service_is_imported_boots() {
    crate::over_an_unconnected_pool()
        .module::<BinnedRacksModule>()
        .build()
        .await
        .expect("the loader `bins` reads is reached");
}

/// Serves nothing that relates: `Bin` is not in its schema.
#[nest_rs_graphql::resolver]
struct RackCountResolver;

#[nest_rs_graphql::operations]
impl RackCountResolver {
    #[query]
    #[public]
    async fn rack_count(&self) -> u32 {
        0
    }
}

#[nest_rs_core::module(
    imports = [nest_rs_graphql::GraphqlModule, BinsModule],
    providers = [RackCountResolver],
)]
struct UnrelatedGraphqlModule;

#[tokio::test]
async fn a_relation_its_schema_does_not_serve_boots_and_counts_its_loader_at_warn() {
    let logs = nest_rs_testing::LogCapture::install();
    crate::over_an_unconnected_pool()
        .module::<UnrelatedGraphqlModule>()
        .module::<nest_rs_seaorm::SeaOrmDatabaseModule>()
        .build()
        .await
        .expect("a relation no served type carries is no defect of this app");
    let line = logs.expect_one("nest_rs::graphql", "dataloaders linked but unreachable");
    assert_eq!(line.level, "warn");
}
