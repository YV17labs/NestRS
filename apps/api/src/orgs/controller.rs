use std::sync::Arc;

use nestrs_http::{controller, crud};

use domain::authn::AuthGuard;
use domain::authz::AppAbilityGuard;
use domain::orgs::{CreateOrgInput, Entity as OrgEntity, Org, OrgsService, UpdateOrgInput};

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
