use nest_rs_core::injectable;
use nest_rs_seaorm::CrudService;

use super::entity::{self, CreateOrg, Entity as Orgs, UpdateOrg};

#[injectable]
#[derive(Default)]
pub struct OrgsService;

impl CrudService for OrgsService {
    type Entity = Orgs;
    type Create = CreateOrg;
    type Update = UpdateOrg;

    fn soft_delete_column() -> Option<entity::Column> {
        Some(entity::Column::DeletedAt)
    }
}
