//! Demo-data seeding for the shared database, organized as per-entity factories.
//!
//! Lives here, not in any consuming app: the database is workspace-shared, so
//! its seed data is workspace infrastructure — like the migrations themselves.
//! Inserts go through SeaQuery (the same dialect the migrations speak), so this
//! depends on no app's entities. Each seeded entity gets a factory under
//! [`factories`] that owns its row shape and insert; [`run`](runner::run) drives
//! them in foreign-key order (orgs before users).

pub mod factories;
mod runner;

pub use runner::run;
