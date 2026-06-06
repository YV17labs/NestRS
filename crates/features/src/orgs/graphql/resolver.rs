use std::sync::Arc;

use nest_rs_graphql::{crud, resolver};

use crate::authz::graphql::GraphqlAuthGuard;
use crate::orgs::{CreateOrgInput, Entity as OrgEntity, Org, OrgsService, UpdateOrgInput};

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
impl OrgsResolver {}
