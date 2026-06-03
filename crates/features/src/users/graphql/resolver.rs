use std::sync::Arc;

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Result};
use nestrs_authz::graphql::authorize;
use nestrs_authz::{Create, Read};
use nestrs_database::graphql::bind;
use nestrs_graphql::{crud, resolver};
use uuid::Uuid;

use crate::authz::graphql::GraphqlAuthGuard;
use crate::orgs::{Org, OrgsServiceById};
use crate::users::core::{
    CreateUserInput, Entity as UserEntity, UpdateUserInput, User, UsersService, UsersServiceByName,
};
use crate::Claims;

#[resolver]
pub struct UsersResolver {
    #[inject]
    users: Arc<UsersService>,
}

#[crud(
    service = users,
    entity = UserEntity,
    output = User,
    create = CreateUserInput,
    update = UpdateUserInput,
)]
#[use_guards(GraphqlAuthGuard)]
impl UsersResolver {
    #[mutation]
    async fn create_user(&self, ctx: &Context<'_>, input: CreateUserInput) -> Result<User> {
        authorize::<Create, UserEntity>(ctx)?;
        let actor = ctx.data::<Claims>()?;
        Ok(self.users.create_in_org(input, actor.org_id).await?)
    }

    #[query]
    async fn user(&self, ctx: &Context<'_>, id: String) -> Result<Option<User>> {
        Ok(bind::<UsersService, Read>(ctx, &id)
            .await?
            .as_ref()
            .map(User::from))
    }

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
