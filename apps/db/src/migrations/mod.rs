//! Schema migrations, applied in order by [`Migrator`].
//!
//! One file per migration (`m<date>_<seq>_<name>.rs`, kept here so they don't
//! crowd the crate root as they accumulate). To add one: drop the file in this
//! folder, declare its `mod` below, and list its `Migration` in
//! [`Migrator::migrations`](migrator::Migrator) — SeaORM tracks applied ones in
//! `seaql_migrations`.

mod m20260526_000000_create_org;
mod m20260526_000001_create_user;
mod migrator;

pub use migrator::{migrate, Migrator};
