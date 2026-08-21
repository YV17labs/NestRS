use std::sync::Arc;

use nest_rs::http::{controller, crud};

use crate::app_authn::AppAuthnGuard;
use crate::app_authz::AppAuthzGuard;
use crate::orgs::{CreateOrg, Entity as OrgEntity, Org, OrgsService, UpdateOrg};

#[controller(path = "/orgs")]
#[use_guards(AppAuthnGuard, AppAuthzGuard)]
pub struct OrgsController {
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
impl OrgsController {}
