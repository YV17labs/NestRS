use std::sync::Arc;

use async_graphql::{Context, Result};
use nest_rs_authz::graphql::authorize;
use nest_rs_authz::{Create, Read};
use nest_rs_seaorm::graphql::bind;
use nest_rs_graphql::{crud, resolver};

use crate::Claims;
use crate::users::{
    CreateUserInput, Entity as UserEntity, UpdateUserInput, User, UsersService,
};

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
}
