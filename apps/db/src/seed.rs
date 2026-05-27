//! Demo-data seeder for the shared database.
//!
//! Lives here, not in any consuming app: the database is workspace-shared, so
//! its seed data is workspace infrastructure — like the migrations themselves.
//! Inserts go through SeaQuery (the same dialect the migrations speak), so this
//! depends on no app's entities. Idempotent via `ON CONFLICT (email) DO NOTHING`,
//! so re-running — or running after `migrate fresh` — is safe.
use anyhow::Result;
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

// Demo orgs the seeded users belong to. Callers scope by these via the
// `x-org-id` header (e.g. apps/api): ACME owns Ada + Grace, Globex owns Alan.
const ORG_ACME: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac3e);
const ORG_GLOBEX: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_61b3);

const DEMO_USERS: [(&str, &str, Uuid); 3] = [
    ("Ada Lovelace", "ada@acme.test", ORG_ACME),
    ("Grace Hopper", "grace@acme.test", ORG_ACME),
    ("Alan Turing", "alan@globex.test", ORG_GLOBEX),
];

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
    OrgId,
    Name,
    Email,
}

/// Returns the count of rows actually inserted (0 when every demo user exists).
pub async fn run(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    for (name, email, org_id) in DEMO_USERS {
        let stmt = Query::insert()
            .into_table(User::Table)
            .columns([User::Id, User::OrgId, User::Name, User::Email])
            .values_panic([
                Uuid::now_v7().into(),
                org_id.into(),
                name.to_owned().into(),
                email.to_owned().into(),
            ])
            .on_conflict(OnConflict::column(User::Email).do_nothing().to_owned())
            .to_owned();
        inserted += db.execute(&stmt).await?.rows_affected();
    }
    Ok(inserted)
}
