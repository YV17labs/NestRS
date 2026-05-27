use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use nestrs_core::{hooks, injectable};
use nestrs_graphql::dataloader;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use uuid::Uuid;
use validator::Validate;

use crate::users::entity::{self, ActiveModel, CreateUserInput, Entity as Users, User};

pub const ORG_ACME: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac3e);

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
