use nest_rs_core::injectable;
use nest_rs_seaorm::CrudService;

use super::entity::{self, CreateOrgInput, Entity as Orgs, UpdateOrgInput};

#[injectable]
#[derive(Default)]
pub struct OrgsService;

impl CrudService for OrgsService {
    type Entity = Orgs;
    type Create = CreateOrgInput;
    type Update = UpdateOrgInput;

    fn soft_delete_column() -> Option<entity::Column> {
        Some(entity::Column::DeletedAt)
    }
}
