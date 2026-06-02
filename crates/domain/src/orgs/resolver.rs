//! GraphQL field resolvers on [`Org`] (`#[field]` — NestJS `@ResolveField`).
//!
//! Root `#[query]` / `#[mutation]` for this feature live in `apps/<app>/orgs/resolver.rs`
//! (`OrgsResolver` there). Same type name, different crate.

use async_graphql::dataloader::DataLoader;
use async_graphql::Result;
use nestrs_graphql::resolver;
use uuid::Uuid;

use crate::orgs::Org;
use crate::users::UsersServiceByOrg;
use crate::users::User;

#[resolver]
pub struct OrgsResolver;

#[resolver]
impl OrgsResolver {
    #[field]
    async fn users(
        &self,
        parent: &Org,
        by_org: &DataLoader<UsersServiceByOrg>,
    ) -> Result<Vec<User>> {
        let id = Uuid::parse_str(&parent.id)?;
        Ok(by_org.load_one(id).await?.unwrap_or_default())
    }
}
