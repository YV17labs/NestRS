use std::sync::Arc;

use nest_rs_graphql::{crud, resolver};

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::orgs::{CreateOrgDto, Entity as OrgEntity, Org, OrgsService, UpdateOrgDto};

#[resolver]
#[use_guards(AuthGuard, AuthzGuard)]
pub struct OrgsResolver {
    #[inject]
    svc: Arc<OrgsService>,
}

#[crud(
    service = svc,
    entity = OrgEntity,
    output = Org,
    create = CreateOrgDto,
    update = UpdateOrgDto,
)]
impl OrgsResolver {}
