//! `#[expose]` on a real entity, in a crate whose one dependency is the
//! umbrella.
//!
//! sea-orm's derives emit a *relative* `sea_orm::`, which the `use` below
//! satisfies — no `sea-orm` line.

#![expect(
    clippy::exhaustive_enums,
    reason = "sea-orm's derives emit the entity's `Column` and `Relation`, as in a developer's crate"
)]

use nest_rs::pipes::validator::ValidationError;
use nest_rs::seaorm::{expose, sea_orm};
use sea_orm::entity::prelude::*;

/// A custom rule, naming `validator`'s error through the framework alone.
fn not_a_placeholder(body: &str) -> Result<(), ValidationError> {
    if body.eq_ignore_ascii_case("todo") {
        return Err(ValidationError::new("placeholder"));
    }
    Ok(())
}

#[cfg_attr(
    not(feature = "graphql"),
    expose(name = "HygieneNote", soft_delete, timestamps)
)]
#[cfg_attr(
    feature = "graphql",
    expose(name = "HygieneNote", graphql, soft_delete, timestamps)
)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(table_name = "hygiene_note")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose(
        input(create, update),
        validate(length(min = 1), custom(function = "not_a_placeholder"))
    )]
    pub body: String,
    #[expose]
    pub created_at: DateTimeWithTimeZone,
    #[expose]
    pub updated_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
}

/// A `#[field_resolver]` answering the wire type: the mask it runs, its probe and
/// the entity's impl through the umbrella.
#[cfg(feature = "graphql")]
pub mod answered {
    use nest_rs::graphql::async_graphql::SimpleObject;
    use nest_rs::graphql::async_graphql::connection::Connection;
    use nest_rs::graphql::{operations, resolver};

    use super::HygieneNote;

    #[derive(SimpleObject)]
    #[graphql(crate = "::nest_rs::graphql::async_graphql", complex)]
    pub struct HygieneNotebook {
        pub id: i32,
    }

    #[resolver]
    pub struct HygieneNotebooksResolver;

    #[operations]
    impl HygieneNotebooksResolver {
        #[query]
        #[public]
        fn notebook(&self) -> HygieneNotebook {
            HygieneNotebook { id: 1 }
        }

        #[field_resolver]
        async fn first_note(&self, _parent: &HygieneNotebook) -> Option<HygieneNote> {
            None
        }

        #[field_resolver]
        async fn notes(&self, _parent: &HygieneNotebook) -> Vec<HygieneNote> {
            Vec::new()
        }

        #[field_resolver]
        async fn page(&self, _parent: &HygieneNotebook) -> Connection<String, HygieneNote> {
            Connection::new(false, false)
        }
    }
}
