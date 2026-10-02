//! [`SeaOrmModule`] — the substrate seam. `SeaOrmModule::for_root(None)` resolves
//! [`SeaOrmConfig`] and opens the one `sea_orm::DatabaseConnection` every SeaORM
//! binding shares: `SeaOrmDatabaseModule` (the `Executor` port and the request
//! layers), `SeaOrmHealthModule` (the health indicator), the worker context.
//! Each binding is imported bare beside it and reads the pool from the
//! container.
//!
//! The crate-root `module.rs` a driver is allowed exactly once: a module *of
//! SeaORM* — the crate's own subject — and not one binding wearing the crate's
//! name.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use sea_orm::{Database, DatabaseConnection};

use crate::SeaOrmConfig;

/// What every binding says when the pool is missing — one sentence, every site,
/// so a reader who forgot the substrate is told the same thing by whichever
/// binding noticed first.
pub(crate) const POOL_REMEDY: &str = "no `sea_orm::DatabaseConnection` in the container — import \
                                      `SeaOrmModule::for_root(None)`, which opens the one pool \
                                      every SeaORM binding shares";

/// The SeaORM substrate. Import [`SeaOrmModule::for_root`] once, then the
/// bindings your app needs beside it — they share the pool it opens.
pub struct SeaOrmModule;

impl SeaOrmModule {
    /// `None` ⇒ load [`SeaOrmConfig`] from `NESTRS_SEAORM__*`; `Some(cfg)` pins
    /// the base those variables overlay, per field.
    ///
    /// A pin is not a test hatch: the deployment's real environment still wins
    /// over it. A test that must not read the ambient environment seeds the
    /// value instead — `App::builder().provide(cfg)` short-circuits the factory.
    pub fn for_root(config: impl Into<Option<SeaOrmConfig>>) -> SeaOrmSetup {
        SeaOrmSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`SeaOrmModule::for_root`]. Resolves the
/// config and queues the async pool factory, so every binding's factory —
/// wherever it falls in `imports = [..]` — finds the pool already built.
pub struct SeaOrmSetup {
    pinned: Option<SeaOrmConfig>,
}

impl DynamicModule for SeaOrmSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<DatabaseConnection, _, _>(|container| async move {
            #[expect(
                clippy::expect_used,
                reason = "provide_feature queued the config's factory in this module's collect"
            )]
            let config = container
                .get::<SeaOrmConfig>()
                .expect("SeaOrmConfig is resolved by ConfigModule::provide_feature");
            connect(&config).await
        })
    }
}

/// Open a standalone connection from `NESTRS_SEAORM__*`, resolving the same
/// [`SeaOrmConfig`] the app's [`SeaOrmModule`] uses. The single connector for
/// tools outside the DI container (`migrate`, `seed`) — a new config knob
/// reaches them without editing each binary.
pub async fn connect_from_env() -> anyhow::Result<DatabaseConnection> {
    use nest_rs_config::Config;
    let config = SeaOrmConfig::load()?;
    connect(&config).await
}

/// The URL may carry credentials, so it is never logged.
async fn connect(config: &SeaOrmConfig) -> anyhow::Result<DatabaseConnection> {
    if config.url.is_empty() {
        anyhow::bail!(
            "{} must be set",
            nest_rs_config::spellings(
                <SeaOrmConfig as nest_rs_config::Namespaced>::NAMESPACE,
                "URL"
            )
        );
    }
    // A config seeded on the builder skips `from_env`, and with it the range
    // the variable is held to: checked again where the budget is spent, before
    // sqlx adds it to a clock.
    if let Some(secs) = config.connect_timeout_secs {
        crate::config::CONNECT_TIMEOUT.check(
            <SeaOrmConfig as nest_rs_config::Namespaced>::NAMESPACE,
            "SeaOrmConfig::connect_timeout_secs",
            std::time::Duration::from_secs(secs),
        )?;
    }
    tracing::info!(
        target: crate::TARGET,
        max_connections = ?config.max_connections,
        "connecting to database"
    );
    Ok(Database::connect(config.connect_options()).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// config-4r2, the seeded path: a config handed to the builder skips
    /// `from_env`, and the top of the range reached sqlx and panicked the boot.
    /// The check where the budget is spent refuses it naming the variable, before
    /// anything is dialled.
    #[tokio::test]
    async fn a_seeded_budget_past_the_ceiling_is_refused_before_sqlx_sees_it() {
        let config = SeaOrmConfig {
            url: "postgres://nobody@127.0.0.1:1/none".to_owned(),
            connect_timeout_secs: Some(u64::MAX),
            ..SeaOrmConfig::default()
        };
        let refused = connect(&config)
            .await
            .expect_err("refused rather than handed to sqlx")
            .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS"))
                && refused.contains("above the 3600s it must be at most"),
            "{refused}"
        );
    }
}
