use nest_rs::core::injectable;
use nest_rs::seaorm::{Creatable, CrudService, Deletable, Updatable};

use super::entity::{self, CreateOrg, Entity as Orgs, UpdateOrg};

#[injectable]
#[derive(Default)]
pub struct OrgsService;

impl CrudService for OrgsService {
    type Entity = Orgs;

    fn soft_delete_column() -> Option<entity::Column> {
        Some(entity::Column::DeletedAt)
    }
}

impl Creatable for OrgsService {
    type Create = CreateOrg;
}

impl Updatable for OrgsService {
    type Update = UpdateOrg;
}

impl Deletable for OrgsService {}
