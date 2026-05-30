use nestrs_core::module;

use crate::authz::AuthzModule;
use crate::orgs::controller::OrgsController;
use crate::orgs::service::OrgsService;

#[module(imports = [AuthzModule], providers = [OrgsService, OrgsController])]
pub struct OrgsModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_orgs_service() {
        use nestrs_auth::{JwtOptions, JwtService};

        let jwt = JwtService::new(JwtOptions::eddsa_verify(identity::DEV_PUBLIC_KEY_PEM))
            .expect("verify-only JwtService from the dev public key");
        let container = OrgsModule::register(
            Container::builder()
                .provide(sea_orm::DatabaseConnection::default())
                .provide_arc(Arc::new(jwt)),
        )
        .build();
        let svc: Option<Arc<OrgsService>> = container.get();
        assert!(svc.is_some());
    }
}
