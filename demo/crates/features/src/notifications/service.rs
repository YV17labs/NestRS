use chrono::TimeDelta;
use nest_rs::authz::Action;
use nest_rs::core::injectable;
use nest_rs::seaorm::{CrudService, Repo, ServiceError};
use sea_orm::{ColumnTrait, QueryFilter, QuerySelect, Set};
use uuid::Uuid;

use super::command::NotifyCommand;
use super::entity::{self, Entity as Notifications};

const RETENTION: TimeDelta = TimeDelta::days(30);

const PURGE_BATCH: u64 = 1_000;

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
        let conn = Repo::<Notifications>::conn()?;
        let model = Repo::<Notifications>::insert_unscoped(active, &conn).await?;
        tracing::debug!(
            target: crate::notifications::TARGET,
            id = %model.id,
            org_id = %model.org_id,
            "notification persisted",
        );
        Ok(())
    }

    pub async fn purge_expired(&self) -> Result<u64, ServiceError> {
        let cutoff = chrono::Utc::now().fixed_offset() - RETENTION;
        let conn = Repo::<Notifications>::conn()?;
        let expired = Repo::<Notifications>::scoped(Action::Delete)
            .filter(entity::Column::CreatedAt.lt(cutoff))
            .limit(PURGE_BATCH)
            .all(&conn)
            .await?;
        let mut purged = 0;
        for model in expired {
            purged += Repo::<Notifications>::delete(model).await?.rows_affected;
        }
        tracing::debug!(
            target: crate::notifications::TARGET,
            purged,
            %cutoff,
            "expired notifications purged",
        );
        Ok(purged)
    }
}
