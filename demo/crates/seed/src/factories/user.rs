use anyhow::Result;
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

use crate::factories::org;

// UUID **v7** sentinels (version `7`, variant `8`) — see `org.rs`: the by-id
// routes reject non-v7 path ids, so seeded rows must be reachable as v7.
pub const ACME_AUTHOR: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac01);
const ACME_USER_2: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac02);
const ACME_USER_3: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac03);
const ACME_ADMIN: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_ac00);
pub const GLOBEX_AUTHOR: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_6101);
const GLOBEX_USER_2: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_6102);
const GLOBEX_ADMIN: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_6100);

/// Pre-computed argon2id PHC hash of the shared demo password `publish-demo`
/// (documented in `demo/README.md`). Pinned as a constant so `db seed` stays
/// idempotent — re-hashing on every run would emit a fresh salt each time,
/// breaking the deterministic-seed invariant (`migrations-seed` rule).
const DEMO_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$zaP0uQPUwd5Zg2ixcO4gbQ$/VaP1hPPCXVNRMARGBGAy8DjXvzrNvxiKQhsfJzRLfU";

/// (id, org, name, email, role, password_hash).
type DemoUser = (
    Uuid,
    Uuid,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

/// Each org carries an `admin` (sees every field of its own rows), a `user`
/// **with** a password (to demonstrate field masking on login), and
/// password-less readers — so the three-curl magic moment (masked list / full
/// list / cross-tenant 403) is playable straight after `db reset`.
const DEMO: [DemoUser; 7] = [
    (
        ACME_ADMIN,
        org::ACME,
        "Acme Admin",
        "admin@acme.test",
        "admin",
        Some(DEMO_PASSWORD_HASH),
    ),
    (
        ACME_AUTHOR,
        org::ACME,
        "Acme Author",
        "acme-user-1@example.test",
        "user",
        Some(DEMO_PASSWORD_HASH),
    ),
    (
        ACME_USER_2,
        org::ACME,
        "Acme Member",
        "acme-user-2@example.test",
        "user",
        None,
    ),
    (
        ACME_USER_3,
        org::ACME,
        "Acme Reader",
        "acme-user-3@example.test",
        "user",
        None,
    ),
    (
        GLOBEX_ADMIN,
        org::GLOBEX,
        "Globex Admin",
        "admin@globex.test",
        "admin",
        Some(DEMO_PASSWORD_HASH),
    ),
    (
        GLOBEX_AUTHOR,
        org::GLOBEX,
        "Globex Author",
        "globex-user-1@example.test",
        "user",
        Some(DEMO_PASSWORD_HASH),
    ),
    (
        GLOBEX_USER_2,
        org::GLOBEX,
        "Globex Member",
        "globex-user-2@example.test",
        "user",
        None,
    ),
];

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
    OrgId,
    Name,
    Email,
    Role,
    PasswordHash,
}

pub async fn seed(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    for (id, org_id, name, email, role, password_hash) in DEMO {
        let stmt = Query::insert()
            .into_table(User::Table)
            .columns([
                User::Id,
                User::OrgId,
                User::Name,
                User::Email,
                User::Role,
                User::PasswordHash,
            ])
            .values_panic([
                id.into(),
                org_id.into(),
                name.to_owned().into(),
                email.to_owned().into(),
                role.to_owned().into(),
                password_hash.map(str::to_owned).into(),
            ])
            .on_conflict(OnConflict::column(User::Email).do_nothing().to_owned())
            .to_owned();
        inserted += db.execute(&stmt).await?.rows_affected();
    }
    Ok(inserted)
}
