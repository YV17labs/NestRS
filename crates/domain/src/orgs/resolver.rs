use async_graphql::dataloader::DataLoader;
use async_graphql::Result;
use nestrs_graphql::resolver;
use uuid::Uuid;

use crate::orgs::Org;
use crate::users::{User, UsersServiceByOrg};

#[resolver]
pub struct OrgRelations;

#[resolver]
impl OrgRelations {
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
