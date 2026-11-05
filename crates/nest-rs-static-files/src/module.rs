//! [`StaticFilesModule`] — the files of a directory or of an embedded folder,
//! served as the HTTP router's fallback at [`StaticFilesConfig::path`].

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::__private::ForRootValue;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Module, Registering, Unnamed};
use nest_rs_http::{BoxEndpoint, HttpFallbackMeta, HttpModule};

use crate::config::StaticFilesConfig;
use crate::disk::DiskRoot;
use crate::embedded::Embedded;
use crate::endpoint::StaticFilesEndpoint;
use crate::files::Files;

/// Serves a folder beside the API: the directory [`StaticFilesConfig::root`]
/// names, or an [`Embedded`] one the binary carries.
///
/// The files answer only what no route and no self-mount claims, outside the
/// global prefix, and publicly: importing the module is the opening, and it
/// imports [`HttpModule`] bare, so the files are served wherever it is
/// imported. A bare import reads `<PREFIX>_STATIC_FILES__*` alone.
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
    /// What the app declares about its static files, in one value: a
    /// [`StaticFilesConfig`] pinned as the base `<PREFIX>_STATIC_FILES__*`
    /// overlays, per field; the [`Embedded`] folder served in place of a
    /// directory — code, with no variable to set it; or both, the folder
    /// first, `(Embedded, StaticFilesConfig)`. The bare import reads those
    /// variables over the defaults and serves the directory they name.
    pub fn for_root(declared: impl ForRootValue<StaticFilesSetup>) -> StaticFilesSetup {
        declared.into_setup()
    }
}

/// [`DynamicModule`] returned by [`StaticFilesModule::for_root`]: resolves
/// [`StaticFilesConfig`] (environment over the pinned base, when one is) and
/// declares the embedded folder, if any.
///
/// `for_root` is the one way to it:
///
/// ```compile_fail,E0277
/// use nest_rs_static_files::{StaticFilesConfig, StaticFilesSetup};
///
/// let second_way = StaticFilesSetup::from(StaticFilesConfig::default());
/// ```
pub struct StaticFilesSetup {
    embedded: Option<Embedded>,
    config: Option<StaticFilesConfig>,
}

impl ForRootValue<StaticFilesSetup> for StaticFilesConfig {
    fn into_setup(self) -> StaticFilesSetup {
        StaticFilesSetup {
            embedded: None,
            config: Some(self),
        }
    }
}

impl ForRootValue<StaticFilesSetup> for Embedded {
    fn into_setup(self) -> StaticFilesSetup {
        StaticFilesSetup {
            embedded: Some(self),
            config: None,
        }
    }
}

impl ForRootValue<StaticFilesSetup> for (Embedded, StaticFilesConfig) {
    fn into_setup(self) -> StaticFilesSetup {
        StaticFilesSetup {
            embedded: Some(self.0),
            config: Some(self.1),
        }
    }
}

/// The remedy a second declared source is refused with.
const ONE_SOURCE: &str = "The static files have one source — declare the embedded folder once, on \
                          the `for_root` that serves it.";

impl DynamicModule for StaticFilesSetup {
    fn module() -> TypeId {
        TypeId::of::<StaticFilesModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = builder.import::<StaticFilesModule>();
        let builder = match self.config.clone() {
            Some(config) => ConfigModule::provide_root(config, builder),
            None => builder,
        };
        // A factory, so the module's register reads it wherever the imports fall.
        match self.embedded {
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
        ConfigModule::provide_feature::<Unnamed, StaticFilesConfig>(builder).import::<HttpModule>()
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        let builder = builder.import::<HttpModule>();
        let snapshot = builder.snapshot();
        let config = match ConfigModule::resolved::<StaticFilesConfig>(&snapshot) {
            Ok(config) => config,
            Err(refusal) => return builder.refuse(refusal),
        };
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
    let endpoint = BoxEndpoint::new(StaticFilesEndpoint::new(
        path.clone(),
        files,
        config.max_age,
        config.spa_fallback,
    ));
    HttpFallbackMeta::new(path, "StaticFilesModule", move |_| endpoint.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_declaration_reaches_the_setup_and_nothing_else_does() {
        let pinned = StaticFilesModule::for_root(StaticFilesConfig::default());
        assert!(pinned.config.is_some() && pinned.embedded.is_none());
    }
}
