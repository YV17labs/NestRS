use std::sync::Arc;

use nestrs_graphql::{crud, resolver};

use domain::orgs::{CreateOrgInput, Entity as OrgEntity, Org, OrgsService, UpdateOrgInput};

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
impl OrgsResolver {}
