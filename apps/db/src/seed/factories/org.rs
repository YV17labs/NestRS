//! Org factory: the demo orgs callers scope by via the `x-org-id` header
//! (e.g. apps/api). Their ids are **fixed** — application code references them —
//! so they are not faker-generated. Idempotent via `ON CONFLICT (id) DO NOTHING`.
use anyhow::Result;
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

/// ACME owns most demo users; Globex is the second tenant. The `user` factory
/// assigns its generated users to these.
pub const ACME: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac3e);
pub const GLOBEX: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_61b3);

const DEMO_ORGS: [(Uuid, &str); 2] = [(ACME, "Acme"), (GLOBEX, "Globex")];

#[derive(DeriveIden)]
enum Org {
    Table,
    Id,
    Name,
}

pub async fn seed(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    for (id, name) in DEMO_ORGS {
        let stmt = Query::insert()
            .into_table(Org::Table)
            .columns([Org::Id, Org::Name])
            .values_panic([id.into(), name.to_owned().into()])
            .on_conflict(OnConflict::column(Org::Id).do_nothing().to_owned())
            .to_owned();
        inserted += db.execute(&stmt).await?.rows_affected();
    }
    Ok(inserted)
}
