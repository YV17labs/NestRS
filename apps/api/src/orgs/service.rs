use std::collections::HashMap;
use std::sync::Arc;

use nestrs_core::injectable;
use nestrs_graphql::dataloader;
use nestrs_orm::CrudService;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::orgs::entity::{self, CreateOrgInput, Entity as Orgs, Org, UpdateOrgInput};

/// The org data API — the single audited gateway to the org table. It inherits
/// the full CRUD from [`CrudService`] (list/page/access/create/update/delete) and
/// adds the `by_id` dataloader that backs `User.org` (a keyed batch that runs
/// outside a request's ambient scope, hence its own connection). The generated
/// `#[crud]` controller and resolver delegate every operation here.
#[injectable]
pub struct OrgsService {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl CrudService for OrgsService {
    type Entity = Orgs;
    type Create = CreateOrgInput;
    type Update = UpdateOrgInput;
}

#[dataloader]
impl OrgsService {
    async fn by_id(&self, ids: &[Uuid]) -> HashMap<Uuid, Org> {
        Orgs::find()
            .filter(entity::Column::Id.is_in(ids.iter().cloned()))
            .all(self.db.as_ref())
            .await
            .unwrap_or_else(|err| {
                tracing::error!(target: "nestrs::loader", error = %err, "by_id loader query failed");
                Vec::new()
            })
            .iter()
            .map(|row| (row.id, Org::from(row)))
            .collect()
    }
}
