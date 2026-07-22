use nest_rs_queue::queue;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyCommand {
    pub org_id: Uuid,
    pub message: String,
}

#[queue(name = "notifications", job = NotifyCommand)]
pub struct NotifyQueue;
