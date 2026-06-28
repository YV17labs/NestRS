use std::sync::Arc;

use async_graphql::{Context, Result};
use nest_rs_authz::{Create, Read};
use nest_rs_graphql::{crud, resolver};
use nest_rs_seaorm::graphql::bind;

use crate::Claims;
use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::users::{CreateUser, Entity as UserEntity, UpdateUser, User, UsersService};

#[resolver]
#[use_guards(AuthGuard, AuthzGuard)]
pub struct UsersResolver {
    #[inject]
    svc: Arc<UsersService>,
}

#[crud(
    service = svc,
    entity = UserEntity,
    output = User,
    create = CreateUser,
    update = UpdateUser,
)]
impl UsersResolver {
    #[mutation]
    #[authorize(Create, UserEntity)]
    async fn create_user(&self, ctx: &Context<'_>, input: CreateUser) -> Result<User> {
        let actor = ctx.data::<Claims>()?;
        let user = self.svc.create_in_org(input, actor.org_id).await?;
        Ok(User::from(&user))
    }

    #[query]
    #[authorize(Read, UserEntity)]
    async fn user(&self, ctx: &Context<'_>, id: String) -> Result<Option<User>> {
        Ok(bind::<UsersService, Read>(ctx, &id)
            .await?
            .as_ref()
            .map(User::from))
    }
}
