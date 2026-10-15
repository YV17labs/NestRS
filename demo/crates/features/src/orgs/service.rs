use nest_rs::core::injectable;
use nest_rs::seaorm::{Creatable, CrudService, Deletable, Updatable};

use super::entity::{CreateOrg, Entity as Orgs, UpdateOrg};

#[injectable]
#[derive(Default)]
pub struct OrgsService;

impl CrudService for OrgsService {
    type Entity = Orgs;
}

impl Creatable for OrgsService {
    type Create = CreateOrg;
}

impl Updatable for OrgsService {
    type Update = UpdateOrg;
}

impl Deletable for OrgsService {}
