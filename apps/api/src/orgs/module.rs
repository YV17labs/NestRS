use nestrs_core::module;

use crate::authz::AuthzModule;
use crate::orgs::controller::OrgsController;
use crate::orgs::resolver::OrgsResolver;
use crate::orgs::service::OrgsService;

#[module(imports = [AuthzModule], providers = [OrgsService, OrgsController, OrgsResolver])]
pub struct OrgsModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    // Non-secret sample EdDSA public key, test-only.
    const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

    #[test]
    fn registers_orgs_service() {
        use nestrs_authn::{JwtOptions, JwtService};

        let jwt = JwtService::new(JwtOptions::eddsa_verify(DEV_PUBLIC_KEY))
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
