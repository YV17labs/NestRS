use nest_rs_core::injectable;
use nest_rs_seaorm::{CrudService, Repo, ServiceError};
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;

use super::command::NotifyCommand;
use super::entity::{self, Entity as Notifications};

#[injectable]
#[derive(Default)]
pub struct NotificationsService;

impl CrudService for NotificationsService {
    type Entity = Notifications;
}

impl NotificationsService {
    pub async fn persist(&self, command: NotifyCommand) -> Result<(), ServiceError> {
        let active = entity::ActiveModel {
            id: Set(Uuid::now_v7()),
            org_id: Set(command.org_id),
            message: Set(command.message),
            created_at: Set(chrono::Utc::now().fixed_offset()),
        };
        let model = active.insert(&Repo::<Notifications>::conn()?).await?;
        tracing::debug!(
            target: "features::notifications",
            id = %model.id,
            org_id = %model.org_id,
            "notification persisted",
        );
        Ok(())
    }
}
