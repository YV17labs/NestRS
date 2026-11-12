use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[expose(
    name = "Notification",
    service = super::service::NotificationsService,
    timestamps
)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(table_name = "notification")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[expose]
    pub message: String,
    #[expose]
    pub created_at: DateTimeWithTimeZone,
}
