use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use nestrs_authz::Action;
use nestrs_core::{hooks, injectable};
use nestrs_database::{CrudService, Repo};
use nestrs_graphql::dataloader;
use poem::error::ResponseError;
use poem::http::StatusCode;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use uuid::Uuid;
use validator::{Validate, ValidationErrors};

use crate::users::entity::{
    self, ActiveModel, CreateUserInput, Entity as Users, UpdateUserInput, User,
};

const DEFAULT_ROLE: &str = "user";

/// A failure creating a user: bad input is a 422, a database error a 500.
/// Implementing `ResponseError` lets a handler return it with `?` and no mapping.
#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error(transparent)]
    Validation(#[from] ValidationErrors),
    #[error(transparent)]
    Db(#[from] DbErr),
}

impl ResponseError for UserError {
    fn status(&self) -> StatusCode {
        match self {
            UserError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            UserError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

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
    pub async fn create_in_org(
        &self,
        input: CreateUserInput,
        org_id: Uuid,
    ) -> Result<User, UserError> {
        input.validate()?;
        let row = ActiveModel {
            id: Set(Uuid::now_v7()),
            org_id: Set(org_id),
            name: Set(input.name),
            email: Set(input.email),
            role: Set(DEFAULT_ROLE.to_owned()),
        };
        let user = row.insert(&Repo::<Users>::conn()?).await?;
        tracing::info!(id = %user.id, %org_id, "user created");
        Ok(User::from(&user))
    }

    pub async fn find_or_create(
        &self,
        email: &str,
        name: &str,
        org_id: Uuid,
    ) -> Result<entity::Model> {
        let conn = Repo::<Users>::conn()?;
        if let Some(user) = Repo::<Users>::scoped(Action::Read)
            .filter(entity::Column::Email.eq(email.to_owned()))
            .one(&conn)
            .await?
        {
            return Ok(user);
        }
        let row = ActiveModel {
            id: Set(Uuid::now_v7()),
            org_id: Set(org_id),
            name: Set(name.to_owned()),
            email: Set(email.to_owned()),
            role: Set(DEFAULT_ROLE.to_owned()),
        };
        let user = row.insert(&conn).await?;
        tracing::info!(target: "nestrs::auth", id = %user.id, %org_id, "provisioned a user");
        Ok(user)
    }
}

#[dataloader]
impl UsersService {
    async fn by_name(&self, names: &[String]) -> HashMap<String, Vec<User>> {
        tracing::debug!(target: "nestrs::loader", count = names.len(), "loading users by name");
        let mut buckets: HashMap<String, Vec<User>> = names
            .iter()
            .map(|name| (name.clone(), Vec::new()))
            .collect();
        let rows = (async {
            Repo::<Users>::scoped(Action::Read)
                .filter(entity::Column::Name.is_in(names.iter().cloned()))
                .all(&Repo::<Users>::conn()?)
                .await
        })
        .await
        .unwrap_or_else(|err: DbErr| {
            tracing::error!(target: "nestrs::loader", error = %err, "by_name loader query failed");
            Vec::new()
        });
        for row in &rows {
            if let Some(bucket) = buckets.get_mut(&row.name) {
                bucket.push(User::from(row));
            }
        }
        buckets
    }

    async fn by_org(&self, org_ids: &[Uuid]) -> HashMap<Uuid, Vec<User>> {
        tracing::debug!(target: "nestrs::loader", count = org_ids.len(), "loading users by org");
        let mut buckets: HashMap<Uuid, Vec<User>> =
            org_ids.iter().map(|org_id| (*org_id, Vec::new())).collect();
        let rows = (async {
            Repo::<Users>::scoped(Action::Read)
                .filter(entity::Column::OrgId.is_in(org_ids.iter().cloned()))
                .all(&Repo::<Users>::conn()?)
                .await
        })
        .await
        .unwrap_or_else(|err: DbErr| {
            tracing::error!(target: "nestrs::loader", error = %err, "by_org loader query failed");
            Vec::new()
        });
        for row in &rows {
            if let Some(bucket) = buckets.get_mut(&row.org_id) {
                bucket.push(User::from(row));
            }
        }
        buckets
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

#[cfg(test)]
mod tests {
    use super::*;

    const ORG_ACME: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac3e);

    fn service() -> UsersService {
        UsersService {
            db: Arc::new(DatabaseConnection::default()),
        }
    }

    #[tokio::test]
    async fn create_rejects_invalid_email() {
        let err = service()
            .create_in_org(
                CreateUserInput {
                    name: "Alice".into(),
                    email: "no-at-sign".into(),
                },
                ORG_ACME,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("email"));
    }

    #[tokio::test]
    async fn create_rejects_empty_name() {
        let err = service()
            .create_in_org(
                CreateUserInput {
                    name: "".into(),
                    email: "alice@example.com".into(),
                },
                ORG_ACME,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("name"));
    }
}
