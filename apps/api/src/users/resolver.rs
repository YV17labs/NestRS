use std::sync::Arc;

use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Result};
use nestrs_authz::Create;
use nestrs_authz_graphql::authorize;
use nestrs_graphql::{crud, resolver};
use uuid::Uuid;

use identity::Claims;

use crate::orgs::entity::Org;
use crate::orgs::service::OrgsServiceById;
use crate::users::entity::{self, CreateUserInput, UpdateUserInput, User};
use crate::users::service::{UsersService, UsersServiceByName};

/// `#[crud]` generates the `users`/`user` queries and the `update_user`/
/// `delete_user` mutations, all delegating to `UsersService` (the same gateway the
/// REST controller uses). Only `create_user` is hand-written — like the
/// controller's `create`, a user's `org_id` comes from the authenticated caller,
/// never the GraphQL input — plus the `#[field]` relations, which `#[crud]` does
/// not generate. The macro keeps all of these and adds the rest.
#[resolver]
pub struct UsersResolver {
    #[inject]
    users: Arc<UsersService>,
}

#[crud(
    service = users,
    entity = entity::Entity,
    output = User,
    create = CreateUserInput,
    update = UpdateUserInput,
)]
impl UsersResolver {
    #[mutation]
    async fn create_user(&self, ctx: &Context<'_>, input: CreateUserInput) -> Result<User> {
        authorize::<Create, entity::Entity>(ctx)?;
        let actor = ctx.data::<Claims>()?;
        let row = self.users.create_in_org(input, actor.org_id).await?;
        Ok(User::from(&row))
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
        let same_name = by_name
            .load_one(parent.name.clone())
            .await?
            .unwrap_or_default();
        // A dataloader runs its batch off the request task, so the ambient
        // ability does not reach it — its read is unscoped. We confine the result
        // to the parent's own org (the parent is already within the caller's
        // scope), so no cross-org row leaks through this field.
        Ok(same_name
            .into_iter()
            .filter(|u| u.id != parent.id && u.org_id == parent.org_id)
            .collect())
    }
}
