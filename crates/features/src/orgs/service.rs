use std::collections::HashMap;

use nest_rs_authz::Action;
use nest_rs_core::injectable;
use nest_rs_seaorm::{CrudService, Repo, ServiceError};
use nest_rs_graphql::dataloader;
use sea_orm::{ColumnTrait, QueryFilter};
use uuid::Uuid;

use super::entity::{self, CreateOrgInput, Entity as Orgs, Org, UpdateOrgInput};

#[injectable]
#[derive(Default)]
pub struct OrgsService;

impl CrudService for OrgsService {
    type Entity = Orgs;
    type Create = CreateOrgInput;
    type Update = UpdateOrgInput;
}

#[dataloader]
impl OrgsService {
    async fn by_id(&self, ids: &[Uuid]) -> Result<HashMap<Uuid, Org>, ServiceError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        tracing::debug!(target: "nest_rs::loader", count = ids.len(), "loading orgs by id");
        let rows = Repo::<Orgs>::scoped(Action::Read)
            .filter(entity::Column::Id.is_in(ids.iter().cloned()))
            .all(&Repo::<Orgs>::conn()?)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.id, Org::from(&row)))
            .collect())
    }
}
