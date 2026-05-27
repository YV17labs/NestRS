//! User factory: demo users spread across the orgs. `fake` supplies the names;
//! emails are derived from a per-org slug so `ON CONFLICT (email) DO NOTHING`
//! keeps re-runs idempotent even though the generated names are random.
use anyhow::Result;
use fake::faker::name::en::Name;
use fake::Fake;
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

use crate::seed::factories::org;

// Per org: its id, the email slug, and how many users to generate.
const DEMO: [(Uuid, &str, usize); 2] = [(org::ACME, "acme", 3), (org::GLOBEX, "globex", 2)];

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
    OrgId,
    Name,
    Email,
}

struct UserRow {
    id: Uuid,
    org_id: Uuid,
    name: String,
    email: String,
}

impl UserRow {
    fn build(org_id: Uuid, slug: &str, n: usize) -> Self {
        Self {
            id: Uuid::now_v7(),
            org_id,
            name: Name().fake(),
            email: format!("{slug}-user-{n}@example.test"),
        }
    }
}

pub async fn seed(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    for (org_id, slug, count) in DEMO {
        for n in 1..=count {
            let row = UserRow::build(org_id, slug, n);
            let stmt = Query::insert()
                .into_table(User::Table)
                .columns([User::Id, User::OrgId, User::Name, User::Email])
                .values_panic([
                    row.id.into(),
                    row.org_id.into(),
                    row.name.into(),
                    row.email.into(),
                ])
                .on_conflict(OnConflict::column(User::Email).do_nothing().to_owned())
                .to_owned();
            inserted += db.execute(&stmt).await?.rows_affected();
        }
    }
    Ok(inserted)
}
