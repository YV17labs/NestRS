use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use nest_rs_authn::{AuthError, CredentialError, burn_verify, hash_password, verify_password};
use nest_rs_authz::Action;
use nest_rs_core::{hooks, injectable};
use nest_rs_graphql::dataloader;
use nest_rs_seaorm::{
    CreateModel, Creatable, CrudService, Deletable, Executor, Repo, ServiceError, Updatable,
    live_condition,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set, SqlErr,
};
use uuid::Uuid;
use validator::Validate;

use super::entity::{self, CreateUser, Entity as Users, UpdateUser, User};

const DEFAULT_ROLE: &str = "user";

#[injectable]
pub struct UsersService {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl CrudService for UsersService {
    type Entity = Users;

    fn soft_delete_column() -> Option<entity::Column> {
        Some(entity::Column::DeletedAt)
    }
}

impl Creatable for UsersService {
    type Create = CreateUser;
}

impl Updatable for UsersService {
    type Update = UpdateUser;
}

impl Deletable for UsersService {}

impl UsersService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    pub async fn authenticate(
        &self,
        email: &str,
        password: &str,
    ) -> Result<entity::Model, AuthError> {
        burn_verify(password);
        let conn = Repo::<Users>::conn().map_err(|e| store_unavailable(email, e))?;
        // Pre-authentication: no principal exists yet, so no ability — the
        // sanctioned unscoped path (see `Repo::unscoped`). Routing through
        // `scoped` here would deny every row on a request executor.
        let user = find_by_email(email, &conn)
            .await
            .map_err(|e| store_unavailable(email, e))?;
        // The store was reachable: from here a failure is a genuine credential
        // mismatch (opaque 401), never confused with the store being down (500).
        verify_credentials(email, user, password).map_err(AuthError::from)
    }

    pub async fn register_with_password(
        &self,
        email: &str,
        name: &str,
        password: &str,
        org_id: Uuid,
    ) -> Result<User, ServiceError> {
        let active = prepare_new_user(
            CreateUser {
                name: name.to_owned(),
                email: email.to_owned(),
            },
            org_id,
            Some(password),
        )?;
        let user = active.insert(&Repo::<Users>::conn()?).await?;
        tracing::info!(target: "features::users", id = %user.id, %org_id, "user registered with password");
        Ok(User::from(&user))
    }

    pub async fn create_in_org(
        &self,
        input: CreateUser,
        org_id: Uuid,
    ) -> Result<entity::Model, ServiceError> {
        let active = prepare_new_user(input, org_id, None)?;
        let user = active.insert(&Repo::<Users>::conn()?).await?;
        tracing::info!(target: "features::users", id = %user.id, %org_id, "user created");
        Ok(user)
    }

    pub async fn find_or_create(
        &self,
        email: &str,
        name: &str,
        org_id: Uuid,
    ) -> Result<entity::Model, ServiceError> {
        let conn = Repo::<Users>::conn()?;
        // Pre-authentication: the OAuth callback resolves the caller before any
        // principal (hence ability) exists, so this reads unscoped — the same
        // sanctioned path as `authenticate`. Routing through `scoped` here would
        // deny every row on the request executor, re-provision on every login,
        // and trip the `UNIQUE(email)` constraint for every returning user.
        if let Some(user) = find_by_email(email, &conn).await? {
            return Ok(user);
        }
        let active = prepare_new_user(
            CreateUser {
                name: name.to_owned(),
                email: email.to_owned(),
            },
            org_id,
            None,
        )?;
        match active.insert(&conn).await {
            Ok(user) => {
                tracing::info!(target: "features::users", id = %user.id, %org_id, "provisioned a user");
                Ok(user)
            }
            // Lost a race with a concurrent first login for this email between
            // the read above and this insert: the row now exists, so re-read it
            // instead of failing the callback (read-then-insert TOCTOU).
            Err(e) if is_unique_violation(&e) => {
                find_by_email(email, &conn).await?.ok_or(ServiceError::Db(e))
            }
            Err(e) => Err(e.into()),
        }
    }
}

/// Shared unscoped by-email lookup for the pre-authentication paths:
/// [`UsersService::authenticate`], and both the initial read and the
/// unique-violation re-read in [`UsersService::find_or_create`].
async fn find_by_email(email: &str, conn: &Executor) -> Result<Option<entity::Model>, DbErr> {
    Repo::<Users>::unscoped()
        .filter(live_condition::<Users>())
        .filter(entity::Column::Email.eq(email.to_owned()))
        .one(conn)
        .await
}

fn is_unique_violation(err: &DbErr) -> bool {
    matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

/// An `AuthError` that renders 500 (the identity store is unreachable), logged
/// at `error` — the sanctioned response for a DB failure on a login path, kept
/// separate from the opaque credential rejection so an outage is never reported
/// to the caller as invalid credentials.
fn store_unavailable(email: &str, err: DbErr) -> AuthError {
    tracing::error!(target: "features::users", %email, error = %err, "credential lookup failed");
    AuthError::Unavailable(err.to_string())
}

pub(crate) fn prepare_new_user(
    input: CreateUser,
    org_id: Uuid,
    password: Option<&str>,
) -> Result<entity::ActiveModel, ServiceError> {
    input.validate()?;
    let password_hash = match password {
        // A hashing failure is not a DB error: surface it as an internal 500
        // (`ServiceError::Internal`) with the source kept for `tracing`.
        Some(plain) => Some(
            hash_password(plain)
                .map_err(|e| ServiceError::internal(format!("password hashing failed: {e}")))?,
        ),
        None => None,
    };
    Ok(active_for_new_user(input, org_id, password_hash))
}

pub(crate) fn verify_credentials(
    email: &str,
    user: Option<entity::Model>,
    password: &str,
) -> Result<entity::Model, CredentialError> {
    let Some(user) = user else {
        burn_verify(password);
        tracing::warn!(target: "features::users", %email, reason = "unknown_email", "login failed");
        return Err(CredentialError);
    };

    let Some(ref hash) = user.password_hash else {
        burn_verify(password);
        tracing::warn!(target: "features::users", %email, reason = "no_password_set", "login failed");
        return Err(CredentialError);
    };

    match verify_password(hash, password) {
        Ok(true) => Ok(user),
        Ok(false) => {
            tracing::warn!(target: "features::users", %email, reason = "bad_password", "login failed");
            Err(CredentialError)
        }
        // The stored hash is unparseable — an operator-visible data fault, not
        // an ordinary wrong password. Stay opaque to the caller (still
        // `CredentialError`) but log at `error` so the corruption is noticed.
        Err(e) => {
            tracing::error!(target: "features::users", %email, error = %e, reason = "unverifiable_hash", "login failed");
            Err(CredentialError)
        }
    }
}

pub(crate) fn active_for_new_user(
    input: CreateUser,
    org_id: Uuid,
    password_hash: Option<String>,
) -> entity::ActiveModel {
    let mut active = input.into_active_model();
    active.org_id = Set(org_id);
    active.role = Set(DEFAULT_ROLE.to_owned());
    if password_hash.is_some() {
        active.password_hash = Set(password_hash);
    }
    active
}

#[dataloader]
impl UsersService {
    async fn by_name(&self, names: &[String]) -> Result<HashMap<String, Vec<User>>, ServiceError> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        tracing::debug!(target: "nest_rs::loader", count = names.len(), "loading users by name");
        let rows = Repo::<Users>::scoped(Action::Read)
            .filter(live_condition::<Users>())
            .filter(entity::Column::Name.is_in(names.iter().cloned()))
            .all(&Repo::<Users>::conn()?)
            .await?;
        Ok(group_users_by_name(names, rows))
    }
}

#[hooks]
impl UsersService {
    #[on_application_shutdown]
    async fn report(&self) -> Result<()> {
        let count = Users::find().count(self.db.as_ref()).await?;
        tracing::info!(target: "features::users", count, "users present at shutdown");
        Ok(())
    }
}

fn group_users_by_name(names: &[String], rows: Vec<entity::Model>) -> HashMap<String, Vec<User>> {
    let mut buckets: HashMap<String, Vec<User>> = names
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();
    for row in rows {
        if let Some(bucket) = buckets.get_mut(&row.name) {
            bucket.push(User::from(&row));
        }
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, org_id: Uuid) -> entity::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::Model {
            id: Uuid::now_v7(),
            org_id,
            name: name.into(),
            email: format!("{name}@example.com"),
            role: "user".into(),
            password_hash: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn default_role_is_user() {
        assert_eq!(DEFAULT_ROLE, "user");
    }

    #[test]
    fn group_by_name_keeps_every_requested_name_as_a_bucket() {
        let names = vec!["ada".to_string(), "bob".into(), "eve".into()];
        let rows = vec![row("ada", Uuid::nil())];
        let buckets = group_users_by_name(&names, rows);

        assert_eq!(buckets.len(), 3, "even empty requests must have a bucket");
        assert_eq!(buckets["ada"].len(), 1);
        assert!(buckets["bob"].is_empty());
        assert!(buckets["eve"].is_empty());
    }

    #[test]
    fn group_by_name_collects_multiple_rows_per_name() {
        let names = vec!["ada".to_string()];
        let rows = vec![row("ada", Uuid::nil()), row("ada", Uuid::nil())];
        let buckets = group_users_by_name(&names, rows);
        assert_eq!(buckets["ada"].len(), 2);
    }

    #[test]
    fn group_by_name_drops_rows_not_in_the_requested_set() {
        let names = vec!["ada".to_string()];
        let rows = vec![row("ada", Uuid::nil()), row("eve", Uuid::nil())];
        let buckets = group_users_by_name(&names, rows);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets["ada"].len(), 1);
    }

    #[test]
    fn group_by_name_returns_an_empty_map_when_no_names_requested() {
        let buckets = group_users_by_name(&[], vec![row("ada", Uuid::nil())]);
        assert!(buckets.is_empty());
    }

    fn input(name: &str, email: &str) -> CreateUser {
        CreateUser {
            name: name.into(),
            email: email.into(),
        }
    }

    fn active_into_get<T: sea_orm::sea_query::ValueType>(
        active: &entity::ActiveModel,
        col: entity::Column,
    ) -> Option<T> {
        use sea_orm::ActiveValue;
        match active.get(col) {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => T::try_from(v).ok(),
            ActiveValue::NotSet => None,
        }
    }

    #[test]
    fn active_for_new_user_sets_org_id_and_default_role_without_password() {
        let org = Uuid::now_v7();
        let active = active_for_new_user(input("ada", "ada@example.com"), org, None);

        assert_eq!(
            active_into_get::<Uuid>(&active, entity::Column::OrgId),
            Some(org)
        );
        assert_eq!(
            active_into_get::<String>(&active, entity::Column::Role).as_deref(),
            Some(DEFAULT_ROLE),
        );
        let pw = active.get(entity::Column::PasswordHash);
        assert!(
            matches!(pw, sea_orm::ActiveValue::NotSet),
            "no password ⇒ NotSet, got {pw:?}",
        );
    }

    #[test]
    fn active_for_new_user_preserves_the_supplied_password_hash() {
        let active = active_for_new_user(
            input("bob", "bob@example.com"),
            Uuid::now_v7(),
            Some("argon2id$mock".into()),
        );
        let pw = active.get(entity::Column::PasswordHash);
        assert!(
            matches!(pw, sea_orm::ActiveValue::Set(_)),
            "password column must be Set when a hash is provided, got {pw:?}",
        );
    }

    #[test]
    fn active_for_new_user_carries_input_fields_into_the_active_model() {
        let active = active_for_new_user(input("eve", "eve@example.com"), Uuid::nil(), None);
        assert_eq!(
            active_into_get::<String>(&active, entity::Column::Name).as_deref(),
            Some("eve"),
        );
        assert_eq!(
            active_into_get::<String>(&active, entity::Column::Email).as_deref(),
            Some("eve@example.com"),
        );
    }

    #[test]
    fn prepare_new_user_rejects_empty_name() {
        let err = prepare_new_user(input("", "ada@example.com"), Uuid::nil(), None)
            .expect_err("empty name should fail validation");
        match err {
            ServiceError::Validation(v) => assert!(v.field_errors().contains_key("name")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn prepare_new_user_rejects_invalid_email() {
        let err = prepare_new_user(input("ada", "not-an-email"), Uuid::nil(), None)
            .expect_err("invalid email should fail validation");
        match err {
            ServiceError::Validation(v) => assert!(v.field_errors().contains_key("email")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn prepare_new_user_without_password_leaves_hash_unset() {
        let active = prepare_new_user(input("ada", "ada@example.com"), Uuid::now_v7(), None)
            .expect("valid input");
        let pw = active.get(entity::Column::PasswordHash);
        assert!(
            matches!(pw, sea_orm::ActiveValue::NotSet),
            "no password ⇒ NotSet, got {pw:?}",
        );
    }

    #[test]
    fn prepare_new_user_with_password_sets_an_argon2id_hash() {
        let org = Uuid::now_v7();
        let active = prepare_new_user(input("ada", "ada@example.com"), org, Some("hunter2"))
            .expect("valid input");
        let s = active_into_get::<Option<String>>(&active, entity::Column::PasswordHash)
            .flatten()
            .expect("password column must be Set");
        assert!(
            s.starts_with("$argon2id$"),
            "password column must hold an argon2id hash, got {s:?}",
        );
    }

    fn user_row(password_hash: Option<String>) -> entity::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::Model {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            name: "ada".into(),
            email: "ada@example.com".into(),
            role: "user".into(),
            password_hash,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn verify_credentials_rejects_absent_user_with_opaque_error() {
        let err = verify_credentials("ghost@example.com", None, "anything")
            .expect_err("no user ⇒ CredentialError");
        assert_eq!(err.to_string(), "invalid credentials");
    }

    #[test]
    fn verify_credentials_rejects_user_without_a_password_hash() {
        let err = verify_credentials("ada@example.com", Some(user_row(None)), "hunter2")
            .expect_err("no hash ⇒ CredentialError");
        assert_eq!(err.to_string(), "invalid credentials");
    }

    #[test]
    fn verify_credentials_rejects_wrong_password() {
        let hash = hash_password("hunter2").expect("hash");
        let err = verify_credentials("ada@example.com", Some(user_row(Some(hash))), "wrong")
            .expect_err("wrong password ⇒ CredentialError");
        assert_eq!(err.to_string(), "invalid credentials");
    }

    #[test]
    fn verify_credentials_accepts_correct_password() {
        let hash = hash_password("hunter2").expect("hash");
        let row = user_row(Some(hash));
        let id = row.id;
        let ok = verify_credentials("ada@example.com", Some(row), "hunter2")
            .expect("correct password ⇒ Ok");
        assert_eq!(ok.id, id);
    }

    fn users_service_disconnected() -> UsersService {
        UsersService::new(Arc::new(DatabaseConnection::default()))
    }

    #[tokio::test]
    async fn by_name_returns_empty_without_touching_the_executor_when_no_keys() {
        let svc = users_service_disconnected();
        let out = svc.by_name(&[]).await.expect("empty keys short-circuit");
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn authenticate_surfaces_a_missing_executor_as_infrastructure_failure() {
        let svc = users_service_disconnected();
        let err = svc
            .authenticate("ada@example.com", "hunter2")
            .await
            .expect_err("no executor ⇒ infrastructure failure, not a credential mismatch");
        assert!(
            matches!(err, AuthError::Unavailable(_)),
            "a store-reach failure must not masquerade as invalid credentials: {err:?}",
        );
    }

    #[tokio::test]
    async fn register_with_password_surfaces_executor_error_as_db_variant() {
        let svc = users_service_disconnected();
        let err = svc
            .register_with_password("ada@example.com", "ada", "hunter2", Uuid::now_v7())
            .await
            .expect_err("no executor ⇒ ServiceError::Db");
        assert!(matches!(err, ServiceError::Db(_)));
        assert_eq!(err.to_string(), "database error");
    }

    #[tokio::test]
    async fn register_with_password_surfaces_validation_error_before_touching_the_db() {
        let svc = users_service_disconnected();
        let err = svc
            .register_with_password("not-an-email", "ada", "hunter2", Uuid::now_v7())
            .await
            .expect_err("invalid email ⇒ Validation");
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_in_org_surfaces_executor_error_as_db_variant() {
        let svc = users_service_disconnected();
        let err = svc
            .create_in_org(
                CreateUser {
                    name: "ada".into(),
                    email: "ada@example.com".into(),
                },
                Uuid::now_v7(),
            )
            .await
            .expect_err("no executor ⇒ ServiceError::Db");
        assert!(matches!(err, ServiceError::Db(_)));
    }

    #[tokio::test]
    async fn create_in_org_surfaces_validation_error_before_touching_the_db() {
        let svc = users_service_disconnected();
        let err = svc
            .create_in_org(
                CreateUser {
                    name: String::new(),
                    email: "ada@example.com".into(),
                },
                Uuid::now_v7(),
            )
            .await
            .expect_err("empty name ⇒ Validation");
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn find_or_create_surfaces_executor_error_as_db_variant() {
        let svc = users_service_disconnected();
        let err = svc
            .find_or_create("ada@example.com", "ada", Uuid::now_v7())
            .await
            .expect_err("no executor ⇒ ServiceError::Db");
        assert!(matches!(err, ServiceError::Db(_)));
    }

    #[test]
    fn new_constructs_a_service_carrying_the_supplied_connection() {
        let db = Arc::new(DatabaseConnection::default());
        let svc = UsersService::new(Arc::clone(&db));
        assert!(Arc::strong_count(&db) >= 2);
        drop(svc);
        assert_eq!(Arc::strong_count(&db), 1);
    }

    #[test]
    fn verify_credentials_rejects_user_with_malformed_stored_hash() {
        let err = verify_credentials(
            "ada@example.com",
            Some(user_row(Some("not-a-real-hash".into()))),
            "hunter2",
        )
        .expect_err("bad stored hash ⇒ CredentialError");
        assert_eq!(err.to_string(), "invalid credentials");
    }
}
