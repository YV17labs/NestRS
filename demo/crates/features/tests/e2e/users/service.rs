use std::sync::Arc;

use features::orgs::ActiveModel as OrgActiveModel;
use features::users::{ActiveModel as UserActiveModel, SocialIdentity, UserRole, UsersService};
use nest_rs_authn::AuthError;
use nest_rs_seaorm::{Executor, with_request_executor};
use nest_rs_testing::EphemeralDatabase;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, Set, Statement, Value};
use uuid::Uuid;

async fn seed_org(conn: &DatabaseConnection, id: Uuid) {
    OrgActiveModel {
        id: Set(id),
        name: Set("Acme".to_owned()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org");
}

async fn seed_user(conn: &DatabaseConnection, org_id: Uuid, email: &str) -> Uuid {
    UserActiveModel {
        id: Set(Uuid::now_v7()),
        org_id: Set(org_id),
        name: Set("Seed".to_owned()),
        email: Set(email.to_owned()),
        role: Set(UserRole::User),
        password_hash: Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed user")
    .id
}

fn github(subject: &str, email: &str, verified: bool) -> SocialIdentity {
    SocialIdentity {
        provider: "github",
        subject: subject.to_owned(),
        email: Some(email.to_owned()),
        email_verified: verified,
        name: Some("Test User".to_owned()),
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

#[tokio::test]
async fn an_unknown_db_role_fails_to_load_rather_than_demoting_silently() {
    use sea_orm::EntityTrait;

    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let org_id = Uuid::now_v7();
    seed_org(conn.as_ref(), org_id).await;

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
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let org_id = Uuid::now_v7();
    seed_org(conn.as_ref(), org_id).await;

    with_request_executor(Executor::Pool((*conn).clone()), async {
        let svc = UsersService::new(Arc::clone(&conn));

        let created = svc
            .resolve_social_identity(&github("42", "ada@first.example", true), org_id)
            .await
            .expect("first social login provisions a user");

        let bob = seed_user(conn.as_ref(), org_id, "ada@second.example").await;
        assert_ne!(bob, created.id);

        let resolved = svc
            .resolve_social_identity(&github("42", "ada@second.example", true), org_id)
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
    })
    .await;
}

#[tokio::test]
async fn a_verified_email_links_to_an_existing_account_exactly_once() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let org_id = Uuid::now_v7();
    seed_org(conn.as_ref(), org_id).await;
    let carol = seed_user(conn.as_ref(), org_id, "carol@example.com").await;

    with_request_executor(Executor::Pool((*conn).clone()), async {
        let svc = UsersService::new(Arc::clone(&conn));

        let first = svc
            .resolve_social_identity(&github("77", "carol@example.com", true), org_id)
            .await
            .expect("a verified email links to the existing account");
        assert_eq!(first.id, carol, "the verified email links to Carol");
        assert_eq!(identity_count(conn.as_ref(), "github", "77").await, 1);
        assert_eq!(
            identity_user_id(conn.as_ref(), "github", "77").await,
            Some(carol),
            "the identity row points at the linked account",
        );

        let second = svc
            .resolve_social_identity(&github("77", "carol@example.com", true), org_id)
            .await
            .expect("second login resolves");
        assert_eq!(second.id, carol);
        assert_eq!(
            identity_count(conn.as_ref(), "github", "77").await,
            1,
            "linking is idempotent across logins",
        );
    })
    .await;
}

#[tokio::test]
async fn an_unverified_email_never_links_to_an_existing_account() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let org_id = Uuid::now_v7();
    seed_org(conn.as_ref(), org_id).await;
    seed_user(conn.as_ref(), org_id, "dave@example.com").await;

    with_request_executor(Executor::Pool((*conn).clone()), async {
        let svc = UsersService::new(Arc::clone(&conn));

        let outcome = svc
            .resolve_social_identity(&github("7", "dave@example.com", false), org_id)
            .await;
        assert!(
            matches!(outcome, Err(AuthError::Failed(_))),
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
    })
    .await;
}

#[tokio::test]
async fn an_unknown_verified_identity_provisions_a_user_and_its_identity() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let org_id = Uuid::now_v7();
    seed_org(conn.as_ref(), org_id).await;

    with_request_executor(Executor::Pool((*conn).clone()), async {
        let svc = UsersService::new(Arc::clone(&conn));

        let user = svc
            .resolve_social_identity(&github("1000", "erin@example.com", true), org_id)
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
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_first_logins_resolve_to_a_single_user() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("ephemeral database");
    let conn = db.connection();
    let org_id = Uuid::now_v7();
    seed_org(conn.as_ref(), org_id).await;

    let pool = (*conn).clone();
    let (c1, c2) = (Arc::clone(&conn), Arc::clone(&conn));
    let (p1, p2) = (pool.clone(), pool);

    let t1 = tokio::spawn(async move {
        with_request_executor(Executor::Pool(p1), async move {
            UsersService::new(c1)
                .resolve_social_identity(&github("999", "frank@example.com", true), org_id)
                .await
        })
        .await
    });
    let t2 = tokio::spawn(async move {
        with_request_executor(Executor::Pool(p2), async move {
            UsersService::new(c2)
                .resolve_social_identity(&github("999", "frank@example.com", true), org_id)
                .await
        })
        .await
    });

    let u1 = t1.await.expect("task 1 joins").expect("resolve 1");
    let u2 = t2.await.expect("task 2 joins").expect("resolve 2");

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
