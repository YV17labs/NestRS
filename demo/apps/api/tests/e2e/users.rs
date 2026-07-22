use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, Action, with_ability};
use nest_rs_seaorm::{Executor, Repo, with_executor};
use poem::http::{StatusCode, header};
use sea_orm::{EntityTrait, IntoActiveModel, Set};
use serde_json::json;
use uuid::Uuid;

use super::harness::*;

#[tokio::test]
async fn users_are_scoped_to_their_org_and_bound_by_id() {
    let (_db, app) = boot().await;

    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "Acme").await;
    let org_b = create_org(&app, &bootstrap, "Globex").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!("Bearer {}", token_for(&org_b, "admin").await);

    let created = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Ada", "email": "ada@acme.test" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let user_a = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    assert!(
        user_names(&app, &token_b).await.is_empty(),
        "org B sees none of org A's users",
    );

    app.http()
        .get(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    app.http()
        .get("/users/018f0000-0000-7000-8000-0000000000ff")
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    app.http()
        .get("/users/not-a-uuid")
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    assert_eq!(user_names(&app, &token_a).await, vec!["Ada".to_string()]);
    let got = app
        .http()
        .get(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .send()
        .await;
    got.assert_status_is_ok();
    assert_eq!(
        got.json().await.value().object().get("name").string(),
        "Ada"
    );
}

mod user_row {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "user")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: Uuid,
        pub name: String,
        pub email: String,
        pub role: String,
        pub password_hash: Option<String>,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[tokio::test]
async fn writes_are_scoped_to_the_callers_ability() {
    let (db, app) = boot().await;

    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "Acme Writes").await;
    let org_b = create_org(&app, &bootstrap, "Globex Writes").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!("Bearer {}", token_for(&org_b, "admin").await);
    let org_b_id = Uuid::parse_str(&org_b).expect("valid org uuid");

    let created = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Ada", "email": "ada-writes@acme.test" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let user_a = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();
    let user_a_id = Uuid::parse_str(&user_a).expect("valid user uuid");

    let patched = app
        .http()
        .patch(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Ada L.", "email": "ada-writes@acme.test" }))
        .send()
        .await;
    patched.assert_status_is_ok();
    assert_eq!(
        patched.json().await.value().object().get("name").string(),
        "Ada L."
    );

    app.http()
        .patch(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_b)
        .body_json(&json!({ "name": "Hijacked", "email": "ada-writes@acme.test" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    app.http()
        .delete(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let conn = db.connection();
    let blocked = Arc::new({
        let mut b = AbilityBuilder::new();
        b.can(Action::Manage, user_row::Entity)
            .when(move |p| p.eq(user_row::Column::OrgId, org_b_id));
        b.build().expect("valid test ability")
    });
    let (update, delete) = with_executor(
        Executor::Pool((*conn).clone()),
        with_ability(blocked, async move {
            let model = user_row::Entity::find_by_id(user_a_id)
                .one(&*conn)
                .await
                .expect("load user A directly")
                .expect("user A exists");
            let mut active = model.clone().into_active_model();
            active.name = Set("Hacked".to_owned());
            let update = Repo::<user_row::Entity>::update(active).await;
            let delete = Repo::<user_row::Entity>::delete(model).await;
            (update, delete)
        }),
    )
    .await;
    assert!(
        matches!(update, Err(sea_orm::DbErr::RecordNotUpdated)),
        "an out-of-scope update touches no row: {update:?}",
    );
    let delete = delete.expect("a delete query runs");
    assert_eq!(
        delete.rows_affected, 0,
        "an out-of-scope delete removes no row",
    );

    let survivor = user_row::Entity::find_by_id(user_a_id)
        .one(&*db.connection())
        .await
        .expect("re-read user A")
        .expect("user A still exists");
    assert_eq!(survivor.name, "Ada L.", "the row was never mutated");

    app.http()
        .delete(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .send()
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.http()
        .get(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let tombstone = user_row::Entity::find_by_id(user_a_id)
        .one(&*db.connection())
        .await
        .expect("re-read user A directly")
        .expect("soft-deleted user row remains in the database");
    assert!(
        tombstone.deleted_at.is_some(),
        "delete stamps deleted_at instead of removing the row",
    );
}

#[tokio::test]
async fn a_plain_user_get_by_id_masks_the_email() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &bootstrap, "Initech").await;

    app.http()
        .post("/users")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_for(&org, "admin").await),
        )
        .body_json(&json!({ "name": "Bob", "email": "bob@initech.test" }))
        .send()
        .await
        .assert_status_is_ok();

    let listed = app
        .http()
        .get("/users")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_for(&org, "admin").await),
        )
        .send()
        .await;
    listed.assert_status_is_ok();
    let user_id = listed
        .json()
        .await
        .value()
        .array()
        .iter()
        .next()
        .expect("one user")
        .object()
        .get("id")
        .string()
        .to_owned();

    let user = format!("Bearer {}", token_for(&org, "user").await);
    let got = app
        .http()
        .get(format!("/users/{user_id}"))
        .header(header::AUTHORIZATION, &user)
        .send()
        .await;
    got.assert_status_is_ok();
    let json = got.json().await;
    let body = json.value().object();
    assert_eq!(body.get("name").string(), "Bob");
    assert!(
        body.get_opt("email").is_none(),
        "a plain user's GET by id masks the email field",
    );
}

#[tokio::test]
async fn a_plain_user_listing_masks_the_email() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &bootstrap, "Initech").await;

    app.http()
        .post("/users")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_for(&org, "admin").await),
        )
        .body_json(&json!({ "name": "Bob", "email": "bob@initech.test" }))
        .send()
        .await
        .assert_status_is_ok();

    let user = format!("Bearer {}", token_for(&org, "user").await);
    let listed = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &user)
        .send()
        .await;
    listed.assert_status_is_ok();
    let body = listed.json().await;
    let first = body
        .value()
        .array()
        .iter()
        .next()
        .expect("one user")
        .object();
    assert_eq!(first.get("name").string(), "Bob");
    assert!(
        first.get_opt("email").is_none(),
        "a plain user's listing masks the email field",
    );
}

#[tokio::test]
async fn a_failed_mutation_persists_nothing() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &bootstrap, "Hooli").await;
    let admin = format!("Bearer {}", token_for(&org, "admin").await);

    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "Ada", "email": "dup@hooli.test" }))
        .send()
        .await
        .assert_status_is_ok();

    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "Grace", "email": "dup@hooli.test" }))
        .send()
        .await
        .assert_status(StatusCode::CONFLICT);

    assert_eq!(user_names(&app, &admin).await, vec!["Ada".to_string()]);
}

#[tokio::test]
async fn a_duplicate_email_create_is_a_conflict_not_a_500() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    create_org(&app, &admin, "SameName").await;
    app.http()
        .post("/orgs")
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "SameName" }))
        .send()
        .await
        .assert_status(StatusCode::CONFLICT);

    let org = create_org(&app, &admin, "Conflict").await;
    let token = format!("Bearer {}", token_for(&org, "admin").await);
    let body = json!({ "name": "Dup", "email": "dup@conflict.test" });
    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &token)
        .body_json(&body)
        .send()
        .await
        .assert_status_is_ok();
    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &token)
        .body_json(&body)
        .send()
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn crud_generated_update_and_delete_round_trip() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    let id = create_org(&app, &admin, "Before").await;

    let patched = app
        .http()
        .patch(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "After" }))
        .send()
        .await;
    patched.assert_status_is_ok();
    assert_eq!(
        patched.json().await.value().object().get("name").string(),
        "After"
    );

    let got = app
        .http()
        .get(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await;
    got.assert_status_is_ok();
    assert_eq!(
        got.json().await.value().object().get("name").string(),
        "After"
    );

    app.http()
        .delete(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await
        .assert_status(StatusCode::NO_CONTENT);

    app.http()
        .get(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn crud_cursor_pagination_walks_the_collection_in_order() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    let mut created = Vec::new();
    for i in 0..5 {
        created.push(create_org(&app, &admin, &format!("Page{i}")).await);
    }

    let mut seen: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    let mut first_page = true;
    loop {
        let path = match &after {
            Some(cursor) => format!("/orgs?first=2&after={cursor}"),
            None => "/orgs?first=2".to_string(),
        };
        let resp = app
            .http()
            .get(&path)
            .header(header::AUTHORIZATION, &admin)
            .send()
            .await;
        resp.assert_status_is_ok();
        if first_page {
            resp.assert_header_exist("x-next-cursor");
            first_page = false;
        }
        let body = resp.json().await;
        let page: Vec<String> = body
            .value()
            .array()
            .iter()
            .map(|o| o.object().get("id").string().to_owned())
            .collect();
        assert!(
            page.len() <= 2,
            "the page respects first=2: got {}",
            page.len()
        );
        if page.is_empty() {
            break;
        }
        after = page.last().cloned();
        seen.extend(page);
        if seen.len() >= created.len() {
            break;
        }
    }

    assert_eq!(seen.len(), 5, "all five orgs are paged through: {seen:?}");
    assert_eq!(
        seen, created,
        "keyset pages preserve ascending-id (chronological) order",
    );
}
