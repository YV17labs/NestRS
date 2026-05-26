use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use nestrs_core::{hooks, injectable};
use nestrs_graphql::dataloader;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Schema, Set,
};
use uuid::Uuid;
use validator::Validate;

use crate::authz::{ORG_ACME, ORG_GLOBEX};
use crate::users::entity::{self, ActiveModel, CreateUserInput, Entity as Users, User};

#[injectable]
pub struct UsersService {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl UsersService {
    pub async fn list(&self, scope: Condition) -> Result<Vec<entity::Model>> {
        Ok(Users::find().filter(scope).all(self.db.as_ref()).await?)
    }

    pub async fn find(&self, id: Uuid) -> Result<Option<entity::Model>> {
        Ok(Users::find_by_id(id).one(self.db.as_ref()).await?)
    }

    pub async fn create(&self, input: CreateUserInput, org_id: Uuid) -> Result<entity::Model> {
        input.validate()?;
        let row = ActiveModel {
            id: Set(Uuid::now_v7()),
            org_id: Set(org_id),
            name: Set(input.name),
            email: Set(input.email),
        };
        Ok(row.insert(self.db.as_ref()).await?)
    }
}

#[dataloader]
impl UsersService {
    async fn by_name(&self, names: &[String]) -> HashMap<String, Vec<User>> {
        let mut buckets: HashMap<String, Vec<User>> = names
            .iter()
            .map(|name| (name.clone(), Vec::new()))
            .collect();
        let rows = Users::find()
            .filter(entity::Column::Name.is_in(names.iter().cloned()))
            .all(self.db.as_ref())
            .await
            .unwrap_or_else(|err| {
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
}

#[hooks]
impl UsersService {
    #[on_module_init]
    async fn migrate_and_seed(&self) -> Result<()> {
        let backend = self.db.get_database_backend();
        let mut create = Schema::new(backend).create_table_from_entity(Users);
        create.if_not_exists();
        self.db.execute(&create).await?;

        if Users::find().one(self.db.as_ref()).await?.is_none() {
            for (name, email, org_id) in [
                ("Ada Lovelace", "ada@acme.test", ORG_ACME),
                ("Grace Hopper", "grace@acme.test", ORG_ACME),
                ("Alan Turing", "alan@globex.test", ORG_GLOBEX),
            ] {
                self.create(
                    CreateUserInput {
                        name: name.to_owned(),
                        email: email.to_owned(),
                    },
                    org_id,
                )
                .await?;
            }
            tracing::info!(target: "nestrs::lifecycle", "seeded users across two orgs");
        }
        Ok(())
    }

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

    fn service() -> UsersService {
        UsersService {
            db: Arc::new(DatabaseConnection::default()),
        }
    }

    #[tokio::test]
    async fn create_rejects_invalid_email() {
        let err = service()
            .create(
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
            .create(
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
