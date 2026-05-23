use nestrs_core::module;

use crate::users::service::UsersService;

// The module wires the service. The resolver self-registers (`#[resolver]`), and
// its DataLoaders self-register too (`#[dataloader]` in service.rs, folded into
// the container by GraphqlModule) — so neither is listed here.
#[module(providers = [UsersService])]
pub struct UsersModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_users_service() {
        let container = UsersModule::register(Container::builder()).build();
        let svc: Option<Arc<UsersService>> = container.get();
        assert!(svc.is_some());
    }
}
