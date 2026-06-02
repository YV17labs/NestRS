use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use nestrs_authn::{burn_verify, hash_password, verify_password};
use nestrs_authz::Action;
use nestrs_core::{hooks, injectable};
use nestrs_database::{CreateModel, CrudService, Repo};
use nestrs_graphql::dataloader;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use uuid::Uuid;
use validator::Validate;

use crate::users::entity::{
    self, CreateUserInput, Entity as Users, UpdateUserInput, User,
};
use crate::users::error::{CredentialError, UserError};

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
    /// Construct with an already-resolved connection (container or tests).
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

        let Some(user) = user else {
            burn_verify(password);
            tracing::warn!(target: "nestrs::auth", %email, "login failed");
            return Err(CredentialError);
        };

        let Some(ref hash) = user.password_hash else {
            burn_verify(password);
            tracing::warn!(target: "nestrs::auth", %email, "login failed");
            return Err(CredentialError);
        };

        if !verify_password(hash, password).unwrap_or(false) {
            tracing::warn!(target: "nestrs::auth", %email, "login failed");
            return Err(CredentialError);
        }
        Ok(user)
    }

    /// Create a user with a local password (email is the login identifier).
    pub async fn register_with_password(
        &self,
        email: &str,
        name: &str,
        password: &str,
        org_id: Uuid,
    ) -> Result<User, UserError> {
        let input = CreateUserInput {
            name: name.to_owned(),
            email: email.to_owned(),
        };
        input.validate()?;
        let password_hash = hash_password(password).map_err(|_| {
            UserError::Db(DbErr::Custom("password hashing failed".into()))
        })?;
        let mut active = input.into_active_model();
        active.org_id = Set(org_id);
        active.role = Set(DEFAULT_ROLE.to_owned());
        active.password_hash = Set(Some(password_hash));
        let user = active.insert(&Repo::<Users>::conn()?).await?;
        tracing::info!(id = %user.id, %org_id, "user registered with password");
        Ok(User::from(&user))
    }

    pub async fn create_in_org(
        &self,
        input: CreateUserInput,
        org_id: Uuid,
    ) -> Result<User, UserError> {
        input.validate()?;
        let mut active = input.into_active_model();
        active.org_id = Set(org_id);
        active.role = Set(DEFAULT_ROLE.to_owned());
        let user = active.insert(&Repo::<Users>::conn()?).await?;
        tracing::info!(id = %user.id, %org_id, "user created");
        Ok(User::from(&user))
    }

    pub async fn find_or_create(
        &self,
        email: &str,
        name: &str,
        org_id: Uuid,
    ) -> Result<entity::Model, UserError> {
        let conn = Repo::<Users>::conn()?;
        if let Some(user) = Repo::<Users>::scoped(Action::Read)
            .filter(entity::Column::Email.eq(email.to_owned()))
            .one(&conn)
            .await?
        {
            return Ok(user);
        }
        let input = CreateUserInput {
            name: name.to_owned(),
            email: email.to_owned(),
        };
        input.validate()?;
        let mut active = input.into_active_model();
        active.org_id = Set(org_id);
        active.role = Set(DEFAULT_ROLE.to_owned());
        let user = active.insert(&conn).await?;
        tracing::info!(target: "nestrs::auth", id = %user.id, %org_id, "provisioned a user");
        Ok(user)
    }
}

#[dataloader]
impl UsersService {
    async fn by_name(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, Vec<User>>, UserError> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        tracing::debug!(target: "nestrs::loader", count = names.len(), "loading users by name");
        let rows = Repo::<Users>::scoped(Action::Read)
            .filter(entity::Column::Name.is_in(names.iter().cloned()))
            .all(&Repo::<Users>::conn()?)
            .await?;
        Ok(group_users_by_name(names, rows))
    }

    async fn by_org(
        &self,
        org_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<User>>, UserError> {
        if org_ids.is_empty() {
            return Ok(HashMap::new());
        }
        tracing::debug!(target: "nestrs::loader", count = org_ids.len(), "loading users by org");
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
        tracing::info!(target: "nestrs::lifecycle", count, "users present at shutdown");
        Ok(())
    }
}

fn group_users_by_name(names: &[String], rows: Vec<entity::Model>) -> HashMap<String, Vec<User>> {
    let mut buckets: HashMap<String, Vec<User>> =
        names.iter().map(|name| (name.clone(), Vec::new())).collect();
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
