use std::sync::Arc;

use async_graphql::{Context, Result};
use nest_rs_authz::graphql::{authorize, masked_output_for};
use nest_rs_authz::{Create, Read};
use nest_rs_graphql::{crud, resolver};
use nest_rs_seaorm::graphql::bind;

use crate::Claims;
use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::users::{CreateUserInput, Entity as UserEntity, UpdateUserInput, User, UsersService};

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
    create = CreateUserInput,
    update = UpdateUserInput,
)]
impl UsersResolver {
    #[mutation]
    async fn create_user(&self, ctx: &Context<'_>, input: CreateUserInput) -> Result<User> {
        authorize::<Create, UserEntity>(ctx)?;
        let actor = ctx.data::<Claims>()?;
        let user = self.svc.create_in_org(input, actor.org_id).await?;
        // Mask through the ambient ability, exactly like the `#[crud]`-generated
        // operations — a hand-written resolver must not leak fields the caller
        // is not granted by returning the wire type unfiltered.
        masked_output_for::<Create, UserEntity, User>(ctx, &user)
    }

    #[query]
    async fn user(&self, ctx: &Context<'_>, id: String) -> Result<Option<User>> {
        match bind::<UsersService, Read>(ctx, &id).await? {
            Some(user) => Ok(Some(masked_output_for::<Read, UserEntity, User>(
                ctx, &user,
            )?)),
            None => Ok(None),
        }
    }
}
