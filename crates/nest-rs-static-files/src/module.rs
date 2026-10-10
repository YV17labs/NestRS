//! [`StaticFilesModule`] — the files of a directory or of an embedded folder,
//! served as the HTTP router's fallback at [`StaticFilesConfig::path`].

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Module, Registering};
use nest_rs_http::{HttpFallbackMeta, join_path, matched};
use poem::Route;

use crate::config::StaticFilesConfig;
use crate::disk::DiskRoot;
use crate::embedded::Embedded;
use crate::endpoint::StaticFilesEndpoint;
use crate::files::Files;

/// Serves a folder beside the API: the directory [`StaticFilesConfig::root`]
/// names, or an [`Embedded`] one the binary carries.
///
/// The files answer only what no route and no self-mount claims, outside the
/// global prefix, and publicly: importing the module is the opening. A bare
/// import reads `<PREFIX>_STATIC_FILES__*` alone.
///
/// ```
/// use nest_rs_core::module;
/// use nest_rs_static_files::{StaticFilesConfig, StaticFilesModule};
///
/// #[module(imports = [StaticFilesModule::for_root(StaticFilesConfig {
///     root: Some("web/dist".into()),
///     spa_fallback: true,
///     ..Default::default()
/// })])]
/// struct AppModule;
/// ```
pub struct StaticFilesModule;

impl StaticFilesModule {
    /// Everything the app says about its static files, in one value — see
    /// [`StaticFilesOptions`]. A config alone, or `None` for the environment
    /// alone, reads like every other module's `for_root`.
    pub fn for_root(options: impl Into<StaticFilesOptions>) -> StaticFilesSetup {
        StaticFilesSetup {
            options: options.into(),
        }
    }
}

/// What an app declares about its static files: the config the environment
/// overlays, and the embedded folder served in place of a directory — code,
/// with no variable to set it.
#[derive(Clone, Debug, Default)]
pub struct StaticFilesOptions {
    /// The base `<PREFIX>_STATIC_FILES__*` overlays, per field. `None` ⇒ the
    /// environment over [`StaticFilesConfig::default`].
    pub config: Option<StaticFilesConfig>,
    /// The folder served from the binary; `None` serves the directory
    /// [`StaticFilesConfig::root`] names.
    pub embedded: Option<Embedded>,
}

impl From<StaticFilesConfig> for StaticFilesOptions {
    fn from(config: StaticFilesConfig) -> Self {
        Some(config).into()
    }
}

impl From<Option<StaticFilesConfig>> for StaticFilesOptions {
    fn from(config: Option<StaticFilesConfig>) -> Self {
        Self {
            config,
            embedded: None,
        }
    }
}

impl From<Embedded> for StaticFilesOptions {
    fn from(embedded: Embedded) -> Self {
        Self {
            config: None,
            embedded: Some(embedded),
        }
    }
}

/// [`DynamicModule`] returned by [`StaticFilesModule::for_root`]: resolves
/// [`StaticFilesConfig`] (environment over the pinned base) and declares the
/// embedded folder, if any.
pub struct StaticFilesSetup {
    options: StaticFilesOptions,
}

/// The remedy a second declared source is refused with.
const ONE_SOURCE: &str = "The static files have one source — declare the embedded folder once, on \
                          the `for_root` that serves it.";

impl DynamicModule for StaticFilesSetup {
    fn module() -> TypeId {
        TypeId::of::<StaticFilesModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(
            self.options.config.clone(),
            builder.import::<StaticFilesModule>(),
        );
        // A factory, so the module's register reads it wherever the imports fall.
        match self.options.embedded {
            Some(embedded) => builder
                .provide_declared_factory::<Embedded, _, _>(ONE_SOURCE, move |_| async move {
                    Ok(embedded)
                }),
            None => builder,
        }
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.import::<StaticFilesModule>()
    }
}

impl Module for StaticFilesModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(None::<StaticFilesConfig>, builder)
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        let snapshot = builder.snapshot();
        #[expect(
            clippy::expect_used,
            reason = "collect queued the config's factory, which resolves before any register"
        )]
        let config = snapshot
            .get::<StaticFilesConfig>()
            .expect("StaticFilesConfig is resolved by ConfigModule::provide_feature");
        let files = match snapshot.get::<Embedded>() {
            Some(embedded) => embedded.files(&config),
            None => DiskRoot::files(&config),
        };
        match files {
            Ok(files) => builder.provide_meta(fallback(&config, files)),
            Err(refusal) => builder.refuse(refusal),
        }
    }
}

/// The router's fallback serving `files` at the configured path.
fn fallback(config: &StaticFilesConfig, files: Files) -> HttpFallbackMeta {
    let (source, root) = match &files {
        Files::Disk(root) => ("disk", Some(root.path().display())),
        Files::Embedded(_) => ("embedded", None),
    };
    tracing::info!(
        target: crate::TARGET,
        path = config.path.as_str(),
        source,
        root = root.map(tracing::field::display),
        "serving static files",
    );
    let path = config.path.clone();
    let endpoint =
        StaticFilesEndpoint::new(path.clone(), files, config.max_age, config.spa_fallback);
    HttpFallbackMeta::new(path.clone(), "StaticFilesModule", move |_, route: Route| {
        route.at(&path, matched(endpoint.clone())).at(
            nest_rs_http::__private::poem_pattern(&join_path(&path, "{*path}")),
            matched(endpoint.clone()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_only_call_site_needs_no_options_literal() {
        let unpinned = StaticFilesModule::for_root(None).options;
        assert!(unpinned.config.is_none() && unpinned.embedded.is_none());

        let pinned = StaticFilesModule::for_root(StaticFilesConfig::default()).options;
        assert!(pinned.config.is_some() && pinned.embedded.is_none());
    }
}
