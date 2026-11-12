use std::sync::Arc;

use nest_rs::graphql::{crud, resolver};

use crate::authz::AuthzGuard;
use crate::orgs::OrgsService;

#[resolver]
#[use_guards(AuthzGuard)]
pub struct OrgsResolver {
    #[inject]
    svc: Arc<OrgsService>,
}

#[crud(service = OrgsService)]
impl OrgsResolver {}
