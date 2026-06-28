use anyhow::Result;
use sea_orm::sea_query::{OnConflict, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

use crate::factories::{org, user};

pub const WELCOME: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_b001);
const ACME_DRAFT: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_b002);
const GLOBEX_LAUNCH: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_b003);

const DEMO: [(Uuid, Uuid, Uuid, &str, &str); 3] = [
    (
        WELCOME,
        org::ACME,
        user::ACME_AUTHOR,
        "Welcome to Publish",
        "Getting started with nestrs and the Publish demo app.",
    ),
    (
        ACME_DRAFT,
        org::ACME,
        user::ACME_AUTHOR,
        "Why Rust for APIs",
        "A short note on type safety, performance, and framework ergonomics.",
    ),
    (
        GLOBEX_LAUNCH,
        org::GLOBEX,
        user::GLOBEX_AUTHOR,
        "Globex launch recap",
        "What we shipped this quarter and what is next on the roadmap.",
    ),
];

#[derive(DeriveIden)]
enum Post {
    Table,
    Id,
    OrgId,
    AuthorId,
    Title,
    Body,
}

pub async fn seed(db: &DatabaseConnection) -> Result<u64> {
    let mut inserted = 0;
    for (id, org_id, author_id, title, body) in DEMO {
        let stmt = Query::insert()
            .into_table(Post::Table)
            .columns([
                Post::Id,
                Post::OrgId,
                Post::AuthorId,
                Post::Title,
                Post::Body,
            ])
            .values_panic([
                id.into(),
                org_id.into(),
                author_id.into(),
                title.to_owned().into(),
                body.to_owned().into(),
            ])
            .on_conflict(OnConflict::column(Post::Id).do_nothing().to_owned())
            .to_owned();
        inserted += db.execute(&stmt).await?.rows_affected();
    }
    Ok(inserted)
}
