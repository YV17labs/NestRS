use std::sync::Arc;
use std::time::Duration;

use features::testing::seed_org;
use features::users::{
    ActiveModel as UserActiveModel, Model, SocialIdentity, UserRole, UsersService,
};
use nest_rs::authn::AuthError;
use nest_rs::authz::{Ability, AbilityBuilder};
use nest_rs::testing::{TestApp, wait_for};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, Set, Statement,
    TransactionTrait, Value,
};
use uuid::Uuid;

use crate::{connection, data_app};

async fn seed_user(conn: &DatabaseConnection, org_id: Uuid, email: &str) -> Uuid {
    features::testing::seed_user(conn, org_id, "Seed", email, UserRole::User).await
}

fn github(subject: &str, email: &str, verified: bool) -> SocialIdentity {
    SocialIdentity {
        provider: "github",
        subject: subject.to_owned(),
        email: Some(email.into()),
        email_verified: verified,
        name: Some("Test User".into()),
    }
}

async fn count(
    conn: &DatabaseConnection,
    sql: &str,
    values: impl IntoIterator<Item = Value>,
) -> i64 {
    let stmt = Statement::from_sql_and_values(conn.get_database_backend(), sql, values);
    conn.query_one_raw(stmt)
        .await
        .expect("count query")
        .expect("count returns a row")
        .try_get::<i64>("", "n")
        .expect("n column")
}

async fn identity_count(conn: &DatabaseConnection, provider: &str, subject: &str) -> i64 {
    count(
        conn,
        "SELECT COUNT(*) AS n FROM user_identity WHERE provider = $1 AND subject = $2",
        [
            Value::from(provider.to_owned()),
            Value::from(subject.to_owned()),
        ],
    )
    .await
}

async fn identity_user_id(
    conn: &DatabaseConnection,
    provider: &str,
    subject: &str,
) -> Option<Uuid> {
    let stmt = Statement::from_sql_and_values(
        conn.get_database_backend(),
        "SELECT user_id FROM user_identity WHERE provider = $1 AND subject = $2",
        [
            Value::from(provider.to_owned()),
            Value::from(subject.to_owned()),
        ],
    );
    conn.query_one_raw(stmt)
        .await
        .expect("query identity")
        .map(|row| row.try_get::<Uuid>("", "user_id").expect("user_id column"))
}

async fn users_with_email(conn: &DatabaseConnection, email: &str) -> i64 {
    count(
        conn,
        "SELECT COUNT(*) AS n FROM \"user\" WHERE email = $1",
        [Value::from(email.to_owned())],
    )
    .await
}

fn visitor() -> Arc<Ability> {
    Arc::new(
        AbilityBuilder::new()
            .build()
            .expect("an empty ability builds"),
    )
}

async fn sign_in(
    app: &TestApp,
    identity: &SocialIdentity,
    org_id: Uuid,
) -> Result<Model, AuthError> {
    app.unit(visitor(), move || async move {
        UsersService.resolve_social_identity(identity, org_id).await
    })
    .await
}

#[tokio::test]
async fn an_unknown_db_role_fails_to_load_rather_than_demoting_silently() {
    use sea_orm::EntityTrait;

    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;

    let rogue = Uuid::now_v7();
    let stmt = Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO \"user\" (id, org_id, name, email, role) VALUES ($1, $2, 'Rogue', 'rogue@acme.test', 'superuser')",
        [Value::from(rogue), Value::from(org_id)],
    );
    conn.execute_raw(stmt)
        .await
        .expect("raw insert of a rogue role bypasses the enum");

    let loaded = features::users::Entity::find_by_id(rogue)
        .one(conn.as_ref())
        .await;
    assert!(
        loaded.is_err(),
        "an unknown role string must fail the typed load (DbErr), never coerce to User: {loaded:?}",
    );
}

#[tokio::test]
async fn a_known_identity_wins_over_a_drifted_email() {
    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;

    let created = sign_in(&app, &github("42", "ada@first.example", true), org_id)
        .await
        .expect("first social login provisions a user");

    let bob = seed_user(conn.as_ref(), org_id, "ada@second.example").await;
    assert_ne!(bob, created.id);

    let resolved = sign_in(&app, &github("42", "ada@second.example", true), org_id)
        .await
        .expect("returning identity resolves");

    assert_eq!(
        resolved.id, created.id,
        "the (provider, subject) identity wins over the drifted email",
    );
    assert_ne!(
        resolved.id, bob,
        "the drifted email must not resolve to Bob"
    );
    assert_eq!(
        identity_count(conn.as_ref(), "github", "42").await,
        1,
        "no second identity row is written for a returning login",
    );
}

#[tokio::test]
async fn a_verified_email_links_to_an_existing_account_exactly_once() {
    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;
    let carol = seed_user(conn.as_ref(), org_id, "carol@example.com").await;

    let first = sign_in(&app, &github("77", "carol@example.com", true), org_id)
        .await
        .expect("a verified email links to the existing account");
    assert_eq!(first.id, carol, "the verified email links to Carol");
    assert_eq!(identity_count(conn.as_ref(), "github", "77").await, 1);
    assert_eq!(
        identity_user_id(conn.as_ref(), "github", "77").await,
        Some(carol),
        "the identity row points at the linked account",
    );

    let second = sign_in(&app, &github("77", "carol@example.com", true), org_id)
        .await
        .expect("second login resolves");
    assert_eq!(second.id, carol);
    assert_eq!(
        identity_count(conn.as_ref(), "github", "77").await,
        1,
        "linking is idempotent across logins",
    );
}

#[tokio::test]
async fn an_unverified_email_never_links_to_an_existing_account() {
    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;
    seed_user(conn.as_ref(), org_id, "dave@example.com").await;

    let outcome = sign_in(&app, &github("7", "dave@example.com", false), org_id).await;
    assert!(
        matches!(outcome, Err(AuthError::Failed { .. })),
        "an unverified email is rejected, never linked: {outcome:?}",
    );

    assert_eq!(
        identity_count(conn.as_ref(), "github", "7").await,
        0,
        "no identity is linked from an unverified email",
    );
    assert_eq!(
        users_with_email(conn.as_ref(), "dave@example.com").await,
        1,
        "the existing account is untouched — no shadow user created",
    );
}

#[tokio::test]
async fn an_unknown_verified_identity_provisions_a_user_and_its_identity() {
    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;

    let user = sign_in(&app, &github("1000", "erin@example.com", true), org_id)
        .await
        .expect("an unknown verified identity provisions a user");

    assert_eq!(user.email, "erin@example.com");
    assert_eq!(
        users_with_email(conn.as_ref(), "erin@example.com").await,
        1,
        "exactly one user is provisioned",
    );
    assert_eq!(
        identity_count(conn.as_ref(), "github", "1000").await,
        1,
        "its identity row is written alongside the user",
    );
    assert_eq!(
        identity_user_id(conn.as_ref(), "github", "1000").await,
        Some(user.id),
        "the identity points at the provisioned user",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_first_logins_resolve_to_a_single_user() {
    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;

    let frank = github("999", "frank@example.com", true);
    let (u1, u2) = tokio::join!(sign_in(&app, &frank, org_id), sign_in(&app, &frank, org_id));
    let u1 = u1.expect("resolve 1");
    let u2 = u2.expect("resolve 2");

    assert_eq!(
        u1.id, u2.id,
        "both concurrent first logins resolve to the same user",
    );
    assert_eq!(
        users_with_email(conn.as_ref(), "frank@example.com").await,
        1,
        "the race provisions exactly one user",
    );
    assert_eq!(
        identity_count(conn.as_ref(), "github", "999").await,
        1,
        "exactly one identity row survives the race",
    );
}

async fn a_statement_waits_on_a_lock(conn: &DatabaseConnection) -> bool {
    count(
        conn,
        "SELECT COUNT(*) AS n FROM pg_stat_activity \
         WHERE datname = current_database() AND wait_event_type = 'Lock'",
        [],
    )
    .await
        > 0
}

async fn sign_in_beside(
    app: &TestApp,
    identity: SocialIdentity,
    org_id: Uuid,
    rival: DatabaseTransaction,
) -> Result<Model, AuthError> {
    let conn = connection(app.container());
    let rival_commits = async {
        wait_for(Duration::from_secs(10), || {
            a_statement_waits_on_a_lock(&conn)
        })
        .await;
        rival.commit().await.expect("the rival login commits");
    };
    let (resolved, ()) = tokio::join!(sign_in(app, &identity, org_id), rival_commits);
    resolved
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mutating_request_linking_an_identity_a_rival_just_linked_commits() {
    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;
    let carol = seed_user(conn.as_ref(), org_id, "carol@example.com").await;

    let rival = conn.begin().await.expect("a rival login");
    rival
        .execute_raw(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "INSERT INTO user_identity (id, user_id, provider, subject, created_at, updated_at) \
             VALUES ($1, $2, 'github', '77', now(), now())",
            [Value::from(Uuid::now_v7()), Value::from(carol)],
        ))
        .await
        .expect("the rival links first");

    let resolved = sign_in_beside(&app, github("77", "carol@example.com", true), org_id, rival)
        .await
        .expect("the met key aborts nothing, so the login resolves and its unit commits");

    assert_eq!(resolved.id, carol);
    assert_eq!(identity_count(conn.as_ref(), "github", "77").await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mutating_request_provisioning_a_user_a_rival_just_created_links_it_and_commits() {
    let app = data_app().await;
    let conn = connection(app.container());
    let org_id = seed_org(conn.as_ref(), "Acme").await;

    let rival = conn.begin().await.expect("a rival login");
    let frank = UserActiveModel {
        id: Set(Uuid::now_v7()),
        org_id: Set(org_id),
        name: Set("Frank".to_owned()),
        email: Set("frank@example.com".to_owned()),
        role: Set(UserRole::User),
        password_hash: Set(None),
        ..Default::default()
    }
    .insert(&rival)
    .await
    .expect("the rival provisions first")
    .id;

    let resolved = sign_in_beside(
        &app,
        github("999", "frank@example.com", true),
        org_id,
        rival,
    )
    .await
    .expect("the met key aborts nothing, so the login resolves and its unit commits");

    assert_eq!(resolved.id, frank);
    assert_eq!(
        users_with_email(conn.as_ref(), "frank@example.com").await,
        1
    );
    assert_eq!(
        identity_user_id(conn.as_ref(), "github", "999").await,
        Some(frank),
    );
}
