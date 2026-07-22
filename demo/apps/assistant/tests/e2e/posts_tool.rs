use nest_rs_testing::mcp::call_tool;
use sea_orm::sea_query::Query;
use sea_orm::{ConnectionTrait, DatabaseConnection, DeriveIden};
use uuid::Uuid;

use super::harness::*;

#[derive(DeriveIden)]
enum Org {
    Table,
    Id,
    Name,
}

#[derive(DeriveIden)]
enum Post {
    Table,
    Id,
    OrgId,
    AuthorId,
    Title,
    Body,
    Status,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
    OrgId,
    Name,
    Email,
    Role,
}

async fn seed_org_with_post(db: &DatabaseConnection, org_name: &str, title: &str) -> Uuid {
    let org_id = Uuid::now_v7();
    let author_id = Uuid::now_v7();

    let org = Query::insert()
        .into_table(Org::Table)
        .columns([Org::Id, Org::Name])
        .values_panic([org_id.into(), org_name.into()])
        .to_owned();
    db.execute(&org).await.expect("seed org");

    let user = Query::insert()
        .into_table(User::Table)
        .columns([User::Id, User::OrgId, User::Name, User::Email, User::Role])
        .values_panic([
            author_id.into(),
            org_id.into(),
            format!("{org_name} author").into(),
            format!("{}@example.com", org_id.simple()).into(),
            "admin".into(),
        ])
        .to_owned();
    db.execute(&user).await.expect("seed author");

    let post = Query::insert()
        .into_table(Post::Table)
        .columns([
            Post::Id,
            Post::OrgId,
            Post::AuthorId,
            Post::Title,
            Post::Body,
            Post::Status,
        ])
        .values_panic([
            Uuid::now_v7().into(),
            org_id.into(),
            author_id.into(),
            title.into(),
            "seeded".into(),
            "draft".into(),
        ])
        .to_owned();
    db.execute(&post).await.expect("seed post");

    org_id
}

#[tokio::test]
async fn a_repo_backed_tool_reads_rows_through_the_ambient_executor() {
    let (db, app) = boot().await;
    let org_id = seed_org_with_post(&db.connection(), "Acme", "acme-only-post").await;

    let body = call_tool(
        app.http(),
        "/posts/mcp",
        "list_posts",
        Some(&bearer_for(&org_id.to_string())),
    )
    .await;

    assert!(
        body.contains("acme-only-post"),
        "the tool must reach the database through `Repo` — no ambient executor \
         would fail closed instead. Body: {body}",
    );
}

#[tokio::test]
async fn a_tool_never_sees_another_orgs_rows() {
    let (db, app) = boot().await;
    let conn = db.connection();
    let acme = seed_org_with_post(&conn, "Acme", "acme-only-post").await;
    seed_org_with_post(&conn, "Globex", "globex-only-post").await;

    let body = call_tool(
        app.http(),
        "/posts/mcp",
        "list_posts",
        Some(&bearer_for(&acme.to_string())),
    )
    .await;

    assert!(
        body.contains("acme-only-post"),
        "the caller's own org row is readable: {body}",
    );
    assert!(
        !body.contains("globex-only-post"),
        "row-level filtering must apply inside the tool body — the tool writes \
         no filter, the ambient ability does. Body: {body}",
    );
}
