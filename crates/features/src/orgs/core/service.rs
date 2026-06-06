use std::collections::HashMap;

use nestrs_authz::Action;
use nestrs_core::injectable;
use nestrs_seaorm::{CrudService, Repo};
use nestrs_graphql::dataloader;
use sea_orm::{ColumnTrait, QueryFilter};
use uuid::Uuid;

use super::entity::{self, CreateOrgInput, Entity as Orgs, Org, UpdateOrgInput};
use super::error::OrgError;

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
    async fn by_id(&self, ids: &[Uuid]) -> Result<HashMap<Uuid, Org>, OrgError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        tracing::debug!(target: "nestrs::loader", count = ids.len(), "loading orgs by id");
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
