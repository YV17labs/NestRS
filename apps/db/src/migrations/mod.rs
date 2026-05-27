//! Schema migrations, applied in order by [`Migrator`].
//!
//! One file per migration (`m<date>_<seq>_<name>.rs`, kept here so they don't
//! crowd the crate root as they accumulate). To add one: drop the file in this
//! folder, declare its `mod` below, and list its `Migration` in
//! [`Migrator::migrations`] — SeaORM tracks applied ones in `seaql_migrations`.
use sea_orm_migration::prelude::*;

mod m20260526_000001_create_user;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260526_000001_create_user::Migration)]
    }
}
