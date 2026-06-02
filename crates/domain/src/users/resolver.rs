//! GraphQL field resolvers on [`User`] (`#[field]` — NestJS `@ResolveField`).
//!
//! Root `#[query]` / `#[mutation]` for this feature live in `apps/<app>/users/resolver.rs`
//! (`UsersResolver` there). Same type name, different crate — like `UsersModule` vs
//! `domain::users::UsersModule`.

use async_graphql::dataloader::DataLoader;
use async_graphql::Result;
use nestrs_graphql::resolver;
use uuid::Uuid;

use crate::orgs::{Org, OrgsServiceById};
use crate::users::UsersServiceByName;
use crate::users::User;

#[resolver]
pub struct UsersResolver;

#[resolver]
impl UsersResolver {
    #[field]
    async fn org(&self, parent: &User, by_id: &DataLoader<OrgsServiceById>) -> Result<Option<Org>> {
        let id = Uuid::parse_str(&parent.org_id)?;
        Ok(by_id.load_one(id).await?)
    }

    #[field]
    async fn namesakes(
        &self,
        parent: &User,
        by_name: &DataLoader<UsersServiceByName>,
    ) -> Result<Vec<User>> {
        let same_name = by_name.load_one(parent.name.clone()).await?.unwrap_or_default();
        Ok(same_name
            .into_iter()
            .filter(|u| u.id != parent.id)
            .collect())
    }
}
