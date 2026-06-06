use std::sync::Arc;

use nest_rs_http::{controller, crud};

use crate::authn::AuthGuard;
use crate::authz::AppAbilityGuard;
use crate::orgs::{CreateOrgInput, Entity as OrgEntity, Org, OrgsService, UpdateOrgInput};

#[controller(path = "/orgs")]
#[use_guards(AuthGuard, AppAbilityGuard)]
pub struct OrgsController {
    #[inject]
    svc: Arc<OrgsService>,
}

#[crud(
    service = svc,
    entity = OrgEntity,
    output = Org,
    create = CreateOrgInput,
    update = UpdateOrgInput,
    paginate = cursor,
)]
impl OrgsController {}
