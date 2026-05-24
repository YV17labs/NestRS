use nestrs_core::module;

use crate::users::controller::UsersController;
use crate::users::service::UsersService;

// The module wires the service and its HTTP controller. The resolver
// self-registers (`#[resolver]`), and its DataLoaders self-register too
// (`#[dataloader]` in service.rs, folded into the container by GraphqlModule) —
// so neither is listed here. The service's lifecycle hooks (`#[hooks]`) likewise
// self-register, via the link-time registry `App` drains at boot.
#[module(providers = [UsersService, UsersController])]
pub struct UsersModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_users_service() {
        // `UsersService` injects a `DatabaseConnection`; seed a disconnected one
        // so registration succeeds without a live database.
        let container = UsersModule::register(
            Container::builder().provide(sea_orm::DatabaseConnection::default()),
        )
        .build();
        let svc: Option<Arc<UsersService>> = container.get();
        assert!(svc.is_some());
    }
}
