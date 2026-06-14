use std::sync::Arc;

use features::users::{CreateUser, UsersService};
use nest_rs_seaorm::ServiceError;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

const ORG_ACME: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac3e);

fn service() -> UsersService {
    UsersService::new(Arc::new(DatabaseConnection::default()))
}

#[tokio::test]
async fn create_rejects_invalid_email() {
    let err = service()
        .create_in_org(
            CreateUser {
                name: "Alice".into(),
                email: "no-at-sign".into(),
            },
            ORG_ACME,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn create_rejects_empty_name() {
    let err = service()
        .create_in_org(
            CreateUser {
                name: "".into(),
                email: "alice@example.com".into(),
            },
            ORG_ACME,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}
