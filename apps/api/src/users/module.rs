use nestrs_core::module;
use nestrs_ws::WsModule;

use crate::authz::AuthzModule;
use crate::users::controller::UsersController;
use crate::users::gateway::UsersGateway;
use crate::users::service::UsersService;

#[module(imports = [WsModule, AuthzModule], providers = [UsersService, UsersController, UsersGateway])]
pub struct UsersModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_users_service() {
        use nestrs_auth::{JwtOptions, JwtService};

        let jwt = JwtService::new(JwtOptions::eddsa_verify(identity::DEV_PUBLIC_KEY_PEM))
            .expect("verify-only JwtService from the dev public key");
        let container = UsersModule::register(
            Container::builder()
                .provide(sea_orm::DatabaseConnection::default())
                .provide_arc(Arc::new(jwt)),
        )
        .build();
        let svc: Option<Arc<UsersService>> = container.get();
        assert!(svc.is_some());
    }
}
