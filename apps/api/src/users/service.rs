use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use nestrs_core::injectable;
use nestrs_graphql::dataloader;
use uuid::{Uuid, Variant, Version};
use validator::Validate;

use crate::users::dto::{CreateUserInput, UserDto};
use crate::users::entity::User;

#[injectable]
#[derive(Default)]
pub struct UsersService {
    users: Mutex<Vec<User>>,
}

impl UsersService {
    pub async fn list(&self) -> Vec<UserDto> {
        self.users
            .lock()
            .expect("users mutex poisoned")
            .iter()
            .map(UserDto::from)
            .collect()
    }

    pub async fn find(&self, id: &str) -> Result<Option<UserDto>> {
        Self::validate_id(id)?;
        Ok(self
            .users
            .lock()
            .expect("users mutex poisoned")
            .iter()
            .find(|u| u.id == id)
            .map(UserDto::from))
    }

    pub async fn create(&self, input: CreateUserInput) -> Result<UserDto> {
        input.validate()?;
        let user = User {
            id: Uuid::now_v7().to_string(),
            name: input.name,
            email: input.email,
        };
        let dto = UserDto::from(&user);
        self.users.lock().expect("users mutex poisoned").push(user);
        Ok(dto)
    }

    fn validate_id(id: &str) -> Result<()> {
        let uuid = Uuid::parse_str(id).map_err(|_| anyhow!("id must be a valid UUID"))?;
        if uuid.get_variant() != Variant::RFC4122 {
            return Err(anyhow!("id must be a RFC 4122 UUID"));
        }
        if uuid.get_version() != Some(Version::SortRand) {
            return Err(anyhow!("id must be a UUID v7"));
        }
        Ok(())
    }
}

// Batched lookups for `#[field]` resolvers — one method per loader. `#[dataloader]`
// generates `UsersServiceByName` + registers its `DataLoader`; with an ORM the
// body becomes a single `WHERE name = ANY($1)` query.
#[dataloader]
impl UsersService {
    async fn by_name(&self, names: &[String]) -> HashMap<String, Vec<UserDto>> {
        let mut buckets: HashMap<String, Vec<UserDto>> = names
            .iter()
            .map(|name| (name.clone(), Vec::new()))
            .collect();
        for user in self.list().await {
            if let Some(bucket) = buckets.get_mut(&user.name) {
                bucket.push(user);
            }
        }
        buckets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_then_list_returns_the_user() {
        let svc = UsersService::default();
        let created = svc
            .create(CreateUserInput {
                name: "Alice".into(),
                email: "alice@example.com".into(),
            })
            .await
            .unwrap();
        let parsed = Uuid::parse_str(&created.id).expect("created id is a uuid");
        assert_eq!(parsed.get_version_num(), 7);

        let all = svc.list().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Alice");
    }

    #[tokio::test]
    async fn find_returns_none_when_uuid_v7_not_found() {
        let svc = UsersService::default();
        let unknown = Uuid::now_v7().to_string();
        assert!(svc.find(&unknown).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn find_rejects_non_uuid() {
        let svc = UsersService::default();
        let err = svc.find("not-a-uuid").await.unwrap_err();
        assert!(err.to_string().contains("UUID"));
    }

    #[tokio::test]
    async fn find_rejects_uuid_other_than_v7() {
        let svc = UsersService::default();
        let v4 = "550e8400-e29b-41d4-a716-446655440000";
        let err = svc.find(v4).await.unwrap_err();
        assert!(err.to_string().contains("v7"));
    }

    #[tokio::test]
    async fn create_rejects_invalid_email() {
        let svc = UsersService::default();
        let err = svc
            .create(CreateUserInput {
                name: "Alice".into(),
                email: "no-at-sign".into(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("email"));
    }

    #[tokio::test]
    async fn create_rejects_empty_name() {
        let svc = UsersService::default();
        let err = svc
            .create(CreateUserInput {
                name: "".into(),
                email: "alice@example.com".into(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("name"));
    }
}
