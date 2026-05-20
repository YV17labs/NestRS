use std::sync::Mutex;

use nestrs_core::injectable;

use crate::users::dto::{CreateUserInput, UserDto};
use crate::users::entity::User;

/// Equivalent of `users.service.ts` in NestJS. In-memory implementation —
/// will be swapped for a repository-backed version once persistence lands.
#[injectable]
pub struct UsersService {
    users: Mutex<Vec<User>>,
    next_id: Mutex<u32>,
}

impl Default for UsersService {
    fn default() -> Self {
        Self {
            users: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
        }
    }
}

impl UsersService {
    pub async fn list(&self) -> Vec<UserDto> {
        self.users
            .lock()
            .unwrap()
            .iter()
            .map(UserDto::from)
            .collect()
    }

    pub async fn find(&self, id: u32) -> Option<UserDto> {
        self.users
            .lock()
            .unwrap()
            .iter()
            .find(|u| u.id == id)
            .map(UserDto::from)
    }

    pub async fn create(&self, input: CreateUserInput) -> UserDto {
        let mut id_lock = self.next_id.lock().unwrap();
        let id = *id_lock;
        *id_lock += 1;
        drop(id_lock);

        let user = User {
            id,
            name: input.name,
            email: input.email,
        };
        let dto = UserDto::from(&user);
        self.users.lock().unwrap().push(user);
        dto
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
            .await;
        assert_eq!(created.id, 1);

        let all = svc.list().await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Alice");
    }

    #[tokio::test]
    async fn find_returns_none_when_missing() {
        let svc = UsersService::default();
        assert!(svc.find(42).await.is_none());
    }

    #[tokio::test]
    async fn create_assigns_incrementing_ids() {
        let svc = UsersService::default();
        let first = svc
            .create(CreateUserInput {
                name: "A".into(),
                email: "a@example.com".into(),
            })
            .await;
        let second = svc
            .create(CreateUserInput {
                name: "B".into(),
                email: "b@example.com".into(),
            })
            .await;
        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
    }
}
