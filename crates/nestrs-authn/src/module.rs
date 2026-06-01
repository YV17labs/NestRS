//! [`AuthnModule`] / [`OAuth2Module`] — make a configured [`JwtService`] /
//! [`OAuth2Client`] injectable everywhere. The analog of NestJS's
//! `JwtModule.register({ ... })`.
//!
//! Each is configured at its import site with **`for_root()`** (no bare form): it
//! routes the load of [`JwtConfig`] / [`OAuth2Config`] through
//! [`ConfigModule::for_feature`] (`NESTRS_AUTHN__*` + the `.env` cascade) and
//! provides its value as global infrastructure (injectable regardless of import
//! order).

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule};

use crate::jwt::{JwtConfig, JwtService};
use crate::oauth::{OAuth2Client, OAuth2Config};

/// Provides the app's [`JwtService`], env-driven via `AuthnModule::for_root()`
/// (loads [`JwtConfig`] from `NESTRS_AUTHN__*`, like `DatabaseModule`). An app signs
/// or only verifies depending on the keys its environment carries and the methods it
/// calls — there is no module-level "issuer" vs "resource server" mode.
pub struct AuthnModule;

impl AuthnModule {
    /// Env-driven: load [`JwtConfig`] from `NESTRS_AUTHN__*` through the config
    /// system, infer the options from the keys present, build the [`JwtService`].
    pub fn for_root() -> AuthnSetup {
        AuthnSetup
    }
}

/// The configured form of [`AuthnModule`]. Provides the [`JwtService`] through the
/// factory phase (global infrastructure, like the database/queue connections).
pub struct AuthnSetup;

impl DynamicModule for AuthnSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::for_feature::<JwtConfig>().collect(builder);
        builder.provide_factory::<JwtService, _, _>(|container| async move {
            let config = container
                .get::<JwtConfig>()
                .expect("JwtConfig is loaded by ConfigModule::for_feature");
            let options = (*config)
                .clone()
                .into_options()
                .map_err(anyhow::Error::new)?;
            JwtService::new(options).map_err(anyhow::Error::new)
        })
    }
}

/// Provides a configured [`OAuth2Client`] for a single provider, injectable as
/// `Arc<OAuth2Client>` by an OAuth [`Strategy`](crate::Strategy), env-driven via
/// `OAuth2Module::for_root()`.
///
/// The flat container keys by type, so one app currently wires one
/// [`OAuth2Client`]; multiple providers would need per-provider newtypes (a
/// future addition).
pub struct OAuth2Module;

impl OAuth2Module {
    /// Env-driven: load [`OAuth2Config`] from `NESTRS_AUTHN__*` through the config
    /// system.
    pub fn for_root() -> OAuth2Setup {
        OAuth2Setup
    }
}

/// The configured form of [`OAuth2Module`].
pub struct OAuth2Setup;

impl DynamicModule for OAuth2Setup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::for_feature::<OAuth2Config>().collect(builder);
        builder.provide_factory::<OAuth2Client, _, _>(|container| async move {
            let config = container
                .get::<OAuth2Config>()
                .expect("OAuth2Config is loaded by ConfigModule::for_feature");
            OAuth2Client::new((*config).clone()).map_err(anyhow::Error::new)
        })
    }
}
