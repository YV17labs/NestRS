use anyhow::Result;
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

use crate::factories::org;

pub const ACME_AUTHOR: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac01);
const ACME_USER_2: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac02);
const ACME_USER_3: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac03);
pub const GLOBEX_AUTHOR: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_6101);
const GLOBEX_USER_2: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_6102);

const DEMO: [(Uuid, Uuid, &str, &str); 5] = [
    (
        ACME_AUTHOR,
        org::ACME,
        "Acme Author",
        "acme-user-1@example.test",
    ),
    (
        ACME_USER_2,
        org::ACME,
        "Acme Member",
        "acme-user-2@example.test",
    ),
    (
        ACME_USER_3,
        org::ACME,
        "Acme Reader",
        "acme-user-3@example.test",
    ),
    (
        GLOBEX_AUTHOR,
        org::GLOBEX,
        "Globex Author",
        "globex-user-1@example.test",
    ),
    (
        GLOBEX_USER_2,
        org::GLOBEX,
        "Globex Member",
        "globex-user-2@example.test",
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
}

pub async fn seed(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    for (id, org_id, name, email) in DEMO {
        let stmt = Query::insert()
            .into_table(User::Table)
            .columns([User::Id, User::OrgId, User::Name, User::Email, User::Role])
            .values_panic([
                id.into(),
                org_id.into(),
                name.to_owned().into(),
                email.to_owned().into(),
                "user".into(),
            ])
            .on_conflict(OnConflict::column(User::Email).do_nothing().to_owned())
            .to_owned();
        inserted += db.execute(&stmt).await?.rows_affected();
    }
    Ok(inserted)
}
