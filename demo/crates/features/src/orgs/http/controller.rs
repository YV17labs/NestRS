use std::sync::Arc;

use nest_rs::http::{controller, crud};

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::orgs::OrgsService;

#[controller(path = "/orgs")]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct OrgsController {
    #[inject]
    svc: Arc<OrgsService>,
}

#[crud(service = svc)]
impl OrgsController {}
