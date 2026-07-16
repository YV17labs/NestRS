//! The `(provider, subject)` social-identity table — how a returning social
//! login is recognized regardless of the user's current email.
//!
//! A satellite of `user`: not `#[expose]`d (it never crosses a wire), no
//! controller/resolver/CRUD. Read and written only through
//! [`UsersService`](super::super::service::UsersService) via `Repo`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "user_identity")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub subject: String,
    /// The provider email captured at link time — an audit fact, never the
    /// lookup key (that is `(provider, subject)`).
    pub email: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
