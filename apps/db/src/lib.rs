//! The workspace's shared database: schema migrations and demo-data seeding.
//!
//! Every app shares one database, so its schema and seed live in a single
//! application — `apps/db` — not under any one app. Migrations are Rust,
//! compiled into the `migrate` binary (shipped in the same container image as
//! the apps); the [`seed`] module backs the `seed` binary. Run them with
//! `just db up` / `just db seed` (or `cargo run -p db --bin migrate -- up`).
//! Migrations live under [`migrations`]; add new ones there.
pub mod seed;

mod migrations;

pub use migrations::{migrate, Migrator};
