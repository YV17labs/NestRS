use nestrs_core::module;

use crate::users::service::UsersService;

/// Feature module — equivalent of `users.module.ts` in NestJS.
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
