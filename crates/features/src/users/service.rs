use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use nest_rs_authn::{CredentialError, burn_verify, hash_password, verify_password};
use nest_rs_authz::Action;
use nest_rs_core::{hooks, injectable};
use nest_rs_seaorm::{CreateModel, CrudService, Repo, ServiceError};
use nest_rs_graphql::dataloader;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use uuid::Uuid;
use validator::Validate;

use super::entity::{self, CreateUserInput, Entity as Users, UpdateUserInput, User};

const DEFAULT_ROLE: &str = "user";

#[injectable]
pub struct UsersService {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl CrudService for UsersService {
    type Entity = Users;
    type Create = CreateUserInput;
    type Update = UpdateUserInput;
}

impl UsersService {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    pub async fn authenticate(
        &self,
        email: &str,
        password: &str,
    ) -> Result<entity::Model, CredentialError> {
        let conn = Repo::<Users>::conn().map_err(|_| CredentialError)?;
        let user = Users::find()
            .filter(entity::Column::Email.eq(email.to_owned()))
            .one(&conn)
            .await
            .map_err(|_| CredentialError)?;
        verify_credentials(email, user, password)
    }

    pub async fn register_with_password(
        &self,
        email: &str,
        name: &str,
        password: &str,
        org_id: Uuid,
    ) -> Result<User, ServiceError> {
        let active = prepare_new_user(
            CreateUserInput {
                name: name.to_owned(),
                email: email.to_owned(),
            },
            org_id,
            Some(password),
        )?;
        let user = active.insert(&Repo::<Users>::conn()?).await?;
        tracing::info!(id = %user.id, %org_id, "user registered with password");
        Ok(User::from(&user))
    }

    pub async fn create_in_org(
        &self,
        input: CreateUserInput,
        org_id: Uuid,
    ) -> Result<User, ServiceError> {
        let active = prepare_new_user(input, org_id, None)?;
        let user = active.insert(&Repo::<Users>::conn()?).await?;
        tracing::info!(id = %user.id, %org_id, "user created");
        Ok(User::from(&user))
    }

    pub async fn find_or_create(
        &self,
        email: &str,
        name: &str,
        org_id: Uuid,
    ) -> Result<entity::Model, ServiceError> {
        let conn = Repo::<Users>::conn()?;
        if let Some(user) = Repo::<Users>::scoped(Action::Read)
            .filter(entity::Column::Email.eq(email.to_owned()))
            .one(&conn)
            .await?
        {
            return Ok(user);
        }
        let active = prepare_new_user(
            CreateUserInput {
                name: name.to_owned(),
                email: email.to_owned(),
            },
            org_id,
            None,
        )?;
        let user = active.insert(&conn).await?;
        tracing::info!(target: "nest_rs::auth", id = %user.id, %org_id, "provisioned a user");
        Ok(user)
    }
}

/// Validate the input, optionally hash the password, and build the
/// `ActiveModel`. Pulled out of `register_with_password` / `create_in_org` /
/// `find_or_create` so validation and hashing failure paths are testable
/// without a DB. Returns `ServiceError::Validation` on bad input and
/// `ServiceError::Db` (wrapping a `Custom` `DbErr`) on hashing failure — the
/// HTTP layer maps both as it would for a real DB error.
pub(crate) fn prepare_new_user(
    input: CreateUserInput,
    org_id: Uuid,
    password: Option<&str>,
) -> Result<entity::ActiveModel, ServiceError> {
    input.validate()?;
    let password_hash = match password {
        Some(plain) => Some(
            hash_password(plain)
                .map_err(|_| ServiceError::Db(DbErr::Custom("password hashing failed".into())))?,
        ),
        None => None,
    };
    Ok(active_for_new_user(input, org_id, password_hash))
}


/// Branch a loaded user against a supplied password into the
/// `Ok(model)` / `Err(CredentialError)` decision the authenticate handler
/// returns. Burns a dummy verify on every miss path so timing does not
/// distinguish "no such email" / "no password set" / "wrong password".
pub(crate) fn verify_credentials(
    email: &str,
    user: Option<entity::Model>,
    password: &str,
) -> Result<entity::Model, CredentialError> {
    let Some(user) = user else {
        burn_verify(password);
        tracing::warn!(target: "nest_rs::auth", %email, "login failed");
        return Err(CredentialError);
    };

    let Some(ref hash) = user.password_hash else {
        burn_verify(password);
        tracing::warn!(target: "nest_rs::auth", %email, "login failed");
        return Err(CredentialError);
    };

    if !verify_password(hash, password).unwrap_or(false) {
        tracing::warn!(target: "nest_rs::auth", %email, "login failed");
        return Err(CredentialError);
    }
    Ok(user)
}

/// Build the `ActiveModel` for a fresh row: validated input, scoped to
/// `org_id`, defaulted to [`DEFAULT_ROLE`], optionally carrying a password
/// hash. Pulled out of `register_with_password` / `create_in_org` /
/// `find_or_create` so the column wiring is testable without a DB.
pub(crate) fn active_for_new_user(
    input: CreateUserInput,
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
            .filter(entity::Column::Name.is_in(names.iter().cloned()))
            .all(&Repo::<Users>::conn()?)
            .await?;
        Ok(group_users_by_name(names, rows))
    }

    async fn by_org(&self, org_ids: &[Uuid]) -> Result<HashMap<Uuid, Vec<User>>, ServiceError> {
        if org_ids.is_empty() {
            return Ok(HashMap::new());
        }
        tracing::debug!(target: "nest_rs::loader", count = org_ids.len(), "loading users by org");
        let rows = Repo::<Users>::scoped(Action::Read)
            .filter(entity::Column::OrgId.is_in(org_ids.iter().cloned()))
            .all(&Repo::<Users>::conn()?)
            .await?;
        Ok(group_users_by_org(org_ids, rows))
    }
}

#[hooks]
impl UsersService {
    #[on_application_shutdown]
    async fn report(&self) -> Result<()> {
        let count = Users::find().count(self.db.as_ref()).await?;
        tracing::info!(target: "nest_rs::lifecycle", count, "users present at shutdown");
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

fn group_users_by_org(org_ids: &[Uuid], rows: Vec<entity::Model>) -> HashMap<Uuid, Vec<User>> {
    let mut buckets: HashMap<Uuid, Vec<User>> =
        org_ids.iter().map(|org_id| (*org_id, Vec::new())).collect();
    for row in rows {
        if let Some(bucket) = buckets.get_mut(&row.org_id) {
            bucket.push(User::from(&row));
        }
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, org_id: Uuid) -> entity::Model {
        entity::Model {
            id: Uuid::now_v7(),
            org_id,
            name: name.into(),
            email: format!("{name}@example.com"),
            role: "user".into(),
            password_hash: None,
        }
    }

    // Pin the new-user default. A change to "admin" would silently
    // escalate every freshly-registered user — high-value sentinel.
    #[test]
    fn default_role_is_user() {
        assert_eq!(DEFAULT_ROLE, "user");
    }

    // The dataloader contract says: one bucket per requested key, in the
    // input set — including the ones the DB returned no row for.

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
        // A row whose name isn't requested is silently dropped — `is_in(names)`
        // shouldn't return them, but the grouper must not panic if it ever
        // does.
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

    #[test]
    fn group_by_org_keeps_every_requested_org_as_a_bucket() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let c = Uuid::now_v7();
        let org_ids = vec![a, b, c];
        let rows = vec![row("ada", a), row("bob", a), row("eve", b)];

        let buckets = group_users_by_org(&org_ids, rows);
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[&a].len(), 2);
        assert_eq!(buckets[&b].len(), 1);
        assert!(buckets[&c].is_empty(), "an org with no users keeps its bucket");
    }

    #[test]
    fn group_by_org_drops_rows_not_in_the_requested_set() {
        let a = Uuid::now_v7();
        let other = Uuid::now_v7();
        let buckets = group_users_by_org(&[a], vec![row("ada", a), row("eve", other)]);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[&a].len(), 1);
    }

    // The `active_for_new_user` helper carries the contract every insert
    // path (`register_with_password`, `create_in_org`, `find_or_create`)
    // depends on. Pin the column wiring here so a refactor of any one
    // caller can't silently drift from the others.

    fn input(name: &str, email: &str) -> CreateUserInput {
        CreateUserInput {
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

        assert_eq!(active_into_get::<Uuid>(&active, entity::Column::OrgId), Some(org));
        assert_eq!(
            active_into_get::<String>(&active, entity::Column::Role).as_deref(),
            Some(DEFAULT_ROLE),
        );
        // No password hash supplied — column stays NotSet (will default at DB
        // level), never overwritten with a stray empty string.
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
        // Just check the variant — value extraction across SeaORM versions
        // is fragile, and "Set" is what the writeback layer needs.
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

    // `prepare_new_user` is the single validate-then-build path every insert
    // route shares. Pin the rejection cases here — a `validator` derive change
    // on the entity must show up as a test failure, not a 500 in prod.

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
        // Argon2id encoded hashes start with `$argon2id$` — a regression to a
        // weaker hasher (or to storing the plaintext) is caught here.
        let s = active_into_get::<Option<String>>(&active, entity::Column::PasswordHash)
            .flatten()
            .expect("password column must be Set");
        assert!(
            s.starts_with("$argon2id$"),
            "password column must hold an argon2id hash, got {s:?}",
        );
    }

    // `verify_credentials` is the pure decision path of `authenticate`. The
    // burn-on-miss is *security-critical* (timing); these tests pin all three
    // miss-or-fail shapes return the same opaque `CredentialError`.

    fn user_row(password_hash: Option<String>) -> entity::Model {
        entity::Model {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            name: "ada".into(),
            email: "ada@example.com".into(),
            role: "user".into(),
            password_hash,
        }
    }

    #[test]
    fn verify_credentials_rejects_absent_user_with_opaque_error() {
        let err = verify_credentials("ghost@example.com", None, "anything")
            .expect_err("no user ⇒ CredentialError");
        // Wire string is the opaque constant — never names the missing user.
        assert_eq!(err.to_string(), "invalid credentials");
    }

    #[test]
    fn verify_credentials_rejects_user_without_a_password_hash() {
        // A user provisioned through OAuth (no local credential) cannot log in
        // via password — `password_hash` is `None`. Same opaque error.
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

    // The dataloader fast-path: an empty `keys` slice must short-circuit to
    // an empty map without touching the executor. Without this, a request
    // with no IDs would call into `Repo` and (correctly) fail with no
    // ambient DbContext — pin the early return.

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
    async fn by_org_returns_empty_without_touching_the_executor_when_no_keys() {
        let svc = users_service_disconnected();
        let out = svc.by_org(&[]).await.expect("empty keys short-circuit");
        assert!(out.is_empty());
    }

    // Without an ambient `DbContext`, `Repo::<Users>::conn()` returns `Err` —
    // every async method here must surface that as its own error variant,
    // never panic. Covers the `?` on `Repo::<…>::conn()` plus the early
    // `map_err(|_| CredentialError)` in `authenticate`.

    #[tokio::test]
    async fn authenticate_returns_credential_error_without_an_ambient_executor() {
        let svc = users_service_disconnected();
        let err = svc
            .authenticate("ada@example.com", "hunter2")
            .await
            .expect_err("no executor ⇒ CredentialError");
        assert_eq!(err.to_string(), "invalid credentials");
    }

    #[tokio::test]
    async fn register_with_password_surfaces_executor_error_as_db_variant() {
        let svc = users_service_disconnected();
        // Validation succeeds, hashing succeeds, then `Repo::conn()?` fails —
        // the resulting `ServiceError::Db` carries the synthetic `DbErr::Custom`
        // and the wire string is the opaque "database error".
        let err = svc
            .register_with_password("ada@example.com", "ada", "hunter2", Uuid::now_v7())
            .await
            .expect_err("no executor ⇒ ServiceError::Db");
        assert!(matches!(err, ServiceError::Db(_)));
        assert_eq!(err.to_string(), "database error");
    }

    #[tokio::test]
    async fn register_with_password_surfaces_validation_error_before_touching_the_db() {
        // An invalid email must surface as `Validation`, not `Db` — even with
        // no executor, because validation runs first.
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
                CreateUserInput {
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
                CreateUserInput {
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

    // `UsersService::new` is what every wiring site uses — verify it builds
    // a usable instance and that the held DB handle stays addressable.
    #[test]
    fn new_constructs_a_service_carrying_the_supplied_connection() {
        let db = Arc::new(DatabaseConnection::default());
        let svc = UsersService::new(Arc::clone(&db));
        // Two clones of the same Arc — strong count is at least the two we
        // hold; the constructor must not have dropped its reference.
        assert!(Arc::strong_count(&db) >= 2);
        drop(svc);
        assert_eq!(Arc::strong_count(&db), 1);
    }

    #[test]
    fn verify_credentials_rejects_user_with_malformed_stored_hash() {
        // A garbled hash in the DB (corruption, manual edit) must surface as
        // "invalid credentials", not crash the request — `verify_password`
        // returns `Err(InvalidHash)` and the helper coerces to `false` via
        // `unwrap_or(false)`.
        let err = verify_credentials(
            "ada@example.com",
            Some(user_row(Some("not-a-real-hash".into()))),
            "hunter2",
        )
        .expect_err("bad stored hash ⇒ CredentialError");
        assert_eq!(err.to_string(), "invalid credentials");
    }
}
