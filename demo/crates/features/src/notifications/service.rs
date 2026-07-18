use nest_rs_core::injectable;
use nest_rs_seaorm::{CrudService, Repo, ServiceError};
use sea_orm::{ActiveModelTrait, Set};
use uuid::Uuid;

use super::command::NotifyCommand;
use super::entity::{self, Entity as Notifications};

/// The notification log's API. Read-only on the wire: it implements only
/// [`CrudService`] (the read half) — **no** `Creatable`/`Updatable`/`Deletable`,
/// so `#[crud]` can generate nothing that mutates over HTTP. The single write
/// path is [`persist`](Self::persist), driven exclusively by the worker in its
/// system context, not by any request.
#[injectable]
#[derive(Default)]
pub struct NotificationsService;

impl CrudService for NotificationsService {
    type Entity = Notifications;
}

impl NotificationsService {
    /// Consumer side (worker): persist a notification. It runs inside the
    /// worker's `WorkerDbContext` executor with **no ambient ability** — system
    /// work is unscoped by design — so the insert goes through `Repo`'s ambient
    /// executor. Fallible: the enqueue-to-persist boundary must not swallow a
    /// `DbErr`, so it propagates the framework's [`ServiceError`].
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
