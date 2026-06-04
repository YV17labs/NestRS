use std::sync::Arc;

use async_graphql::Result;
use async_graphql::dataloader::DataLoader;
use nestrs_graphql::{crud, resolver};
use uuid::Uuid;

use crate::authz::graphql::GraphqlAuthGuard;
use crate::orgs::core::{CreateOrgInput, Entity as OrgEntity, Org, OrgsService, UpdateOrgInput};
use crate::users::{User, UsersServiceByOrg};

#[resolver]
pub struct OrgsResolver {
    #[inject]
    svc: Arc<OrgsService>,
}

#[crud(
    service = svc,
    entity = OrgEntity,
    output = Org,
    create = CreateOrgInput,
    update = UpdateOrgInput,
)]
#[use_guards(GraphqlAuthGuard)]
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
