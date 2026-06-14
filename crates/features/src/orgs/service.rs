use nest_rs_core::injectable;
use nest_rs_seaorm::CrudService;

use super::entity::{self, CreateOrgDto, Entity as Orgs, UpdateOrgDto};

#[injectable]
#[derive(Default)]
pub struct OrgsService;

impl CrudService for OrgsService {
    type Entity = Orgs;
    type Create = CreateOrgDto;
    type Update = UpdateOrgDto;

    fn soft_delete_column() -> Option<entity::Column> {
        Some(entity::Column::DeletedAt)
    }
}
