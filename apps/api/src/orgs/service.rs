use std::collections::HashMap;

use nestrs_authz::Action;
use nestrs_core::injectable;
use nestrs_graphql::dataloader;
use nestrs_orm::{CrudService, Repo};
use sea_orm::{ColumnTrait, DbErr, QueryFilter};
use uuid::Uuid;

use crate::orgs::entity::{self, CreateOrgInput, Entity as Orgs, Org, UpdateOrgInput};

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
    async fn by_id(&self, ids: &[Uuid]) -> HashMap<Uuid, Org> {
        tracing::debug!(target: "nestrs::loader", count = ids.len(), "loading orgs by id");
        let rows = (async {
            Repo::<Orgs>::scoped(Action::Read)
                .filter(entity::Column::Id.is_in(ids.iter().cloned()))
                .all(&Repo::<Orgs>::conn()?)
                .await
        })
        .await
        .unwrap_or_else(|err: DbErr| {
            tracing::error!(target: "nestrs::loader", error = %err, "by_id loader query failed");
            Vec::new()
        });
        rows.iter().map(|row| (row.id, Org::from(row))).collect()
    }
}
