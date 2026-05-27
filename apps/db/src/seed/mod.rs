//! Demo-data seeding for the shared database, organized as per-entity factories.
//!
//! Lives here, not in any consuming app: the database is workspace-shared, so
//! its seed data is workspace infrastructure — like the migrations themselves.
//! Inserts go through SeaQuery (the same dialect the migrations speak), so this
//! depends on no app's entities. Each seeded entity gets a factory under
//! [`factories`] that owns its row shape and insert; [`run`] drives them in
//! foreign-key order (orgs before users).
use anyhow::Result;
use sea_orm::DatabaseConnection;

pub mod factories;

/// Seeds the demo data and returns the number of rows actually inserted (0 when
/// everything already exists — re-running, or running after `migrate fresh`, is
/// safe). Orgs are seeded before users so the `user.org_id` foreign key resolves.
pub async fn run(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    inserted += factories::org::seed(db).await?;
    inserted += factories::user::seed(db).await?;
    Ok(inserted)
}
