use std::sync::Arc;

use nest_rs_graphql::{crud, resolver};

use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;
use crate::orgs::{CreateOrg, Entity as OrgEntity, Org, OrgsService, UpdateOrg};

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
    create = CreateOrg,
    update = UpdateOrg,
)]
impl OrgsResolver {}
