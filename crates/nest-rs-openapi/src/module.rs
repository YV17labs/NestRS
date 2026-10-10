//! `OpenApiModule` — self-mounts `/api` (Swagger UI) + `/api-json` (the document)
//! over the HTTP transport. Both endpoints are **public** (`EdgePosture::Exempt`).

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, Container, ContainerBuilder, DynamicModule, Registering};
use nest_rs_http::{HttpEndpointMeta, join_path, matched, version_path};
use poem::{Route, get};

use crate::config::OpenApiConfig;
use crate::document::{Reported, build_document, versioned_documents};
use crate::ui;

// Must stay siblings, assets under `DOCS_PATH`: `index.html` resolves them relatively.
const DOCS_PATH: &str = "/api";
const SPEC_PATH: &str = "/api-json";
const CSS_PATH: &str = "/api/swagger-ui.css";
const BUNDLE_PATH: &str = "/api/swagger-ui-bundle.js";
const PRESET_PATH: &str = "/api/swagger-ui-standalone-preset.js";
const INITIALIZER_PATH: &str = "/api/swagger-initializer.js";
const VERSIONED_SPEC_PATTERN: &str = "/api-json/*version";

/// Add to a `#[module(imports = [...])]` to expose `GET /api-json` (the OpenAPI
/// 3.1 document) and `GET /api` (bundled Swagger UI). Wire it with
/// `OpenApiModule::for_root(None)`; configuration loads from `<PREFIX>_OPENAPI__*`.
///
/// Both endpoints are public, so they are served in a development or test
/// profile alone until `<PREFIX>_OPENAPI__ENABLED=true` (or a pinned
/// `OpenApiConfig { enabled: true, .. }`) opens them — see [`OpenApiConfig`].
pub struct OpenApiModule;

impl OpenApiModule {
    /// Pass `None` to load [`OpenApiConfig`] from `<PREFIX>_OPENAPI__*`, or an
    /// `OpenApiConfig` to pin as the base those variables overlay, per field.
    pub fn for_root(config: impl Into<Option<OpenApiConfig>>) -> OpenApiSetup {
        OpenApiSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`OpenApiModule::for_root`]. Registers the
/// [`OpenApiConfig`] and self-mounts the `/api-json` + `/api` endpoints.
pub struct OpenApiSetup {
    pinned: Option<OpenApiConfig>,
}

impl DynamicModule for OpenApiSetup {
    fn module() -> TypeId {
        TypeId::of::<OpenApiModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        #[expect(
            clippy::expect_used,
            reason = "provide_feature queued the config's factory in this module's collect"
        )]
        let config = builder
            .snapshot()
            .get::<OpenApiConfig>()
            .expect("OpenApiConfig is resolved by ConfigModule::provide_feature");
        register(builder, (*config).clone())
    }
}

fn register(builder: ContainerBuilder, options: OpenApiConfig) -> ContainerBuilder {
    if !options.enabled {
        tracing::info!(
            target: nest_rs_http::target::ROUTES,
            docs_path = DOCS_PATH,
            spec_path = SPEC_PATH,
            "openapi documentation disabled",
        );
        return builder;
    }
    builder.provide_meta(
        HttpEndpointMeta::new(DOCS_PATH, "openapi", move |container, route: Route| {
            // One ledger for every document, so a route collision is reported once.
            let mut reported = Reported::default();
            let default = spec(container, &options, None, &mut reported);
            if options.emit_document {
                let dest = options.document_path.clone();
                let contents = format!("{default}\n");
                tokio::task::spawn_blocking(move || match std::fs::write(&dest, &contents) {
                    Ok(()) => tracing::info!(
                        target: crate::TARGET,
                        path = %dest.display(),
                        bytes = contents.len(),
                        "wrote OpenAPI document",
                    ),
                    Err(err) => tracing::warn!(
                        target: crate::TARGET,
                        path = %dest.display(),
                        error = %nest_rs_core::error_message(&err),
                        "failed to write OpenAPI document",
                    ),
                });
            }
            let mut route = route
                .at(SPEC_PATH, matched(get(ui::spec_endpoint(default))))
                .at(DOCS_PATH, matched(get(ui::swagger_index)))
                .at(CSS_PATH, matched(get(ui::swagger_css)))
                .at(BUNDLE_PATH, matched(get(ui::swagger_bundle)))
                .at(PRESET_PATH, matched(get(ui::swagger_preset)))
                .at(INITIALIZER_PATH, matched(get(ui::swagger_initializer)));
            // OpenAPI keys operations by path, so a version the path does not
            // carry needs a document of its own.
            for version in versioned_documents(container) {
                let spec = spec(container, &options, Some(&version), &mut reported);
                route = route.at(
                    document_path(&version),
                    matched(get(ui::spec_endpoint(spec))),
                );
            }
            route
        })
        // Every path the closure registers beyond `DOCS_PATH`, or a versioned
        // catch-all controller answers `/api-json`, a sibling outside its subtree.
        .also_mounts([
            SPEC_PATH,
            VERSIONED_SPEC_PATTERN,
            CSS_PATH,
            BUNDLE_PATH,
            PRESET_PATH,
            INITIALIZER_PATH,
        ])
        .exempt(),
    )
}

fn spec(
    container: &Container,
    options: &OpenApiConfig,
    claims: Option<&str>,
    reported: &mut Reported,
) -> String {
    let document = build_document(container, options, claims, reported);
    serde_json::to_string_pretty(&document).unwrap_or_else(|_| document.to_string())
}

fn document_path(version: &str) -> String {
    join_path(SPEC_PATH, &version_path(Some(version), "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::Discovery;
    use nest_rs_http::HttpConfig;

    fn mount_count(enabled: bool) -> usize {
        let builder = register(
            ContainerBuilder::default(),
            OpenApiConfig {
                enabled,
                ..OpenApiConfig::default()
            },
        );
        Discovery::new(&builder.snapshot())
            .meta::<HttpEndpointMeta>()
            .len()
    }

    #[test]
    fn enabled_self_mounts_the_documentation_edge() {
        assert_eq!(
            mount_count(true),
            1,
            "enabled must self-mount the docs edge"
        );
    }

    #[test]
    fn disabled_self_mounts_nothing() {
        assert_eq!(
            mount_count(false),
            0,
            "disabled must mount neither /api nor /api-json — no public schema",
        );
    }

    fn deployment(config: HttpConfig, versions: &'static [&'static str]) -> Container {
        nest_rs_core::Container::builder()
            .provide(config)
            .provide_meta(nest_rs_http::HttpControllerMeta::new(
                "PostsController",
                "posts",
                "/posts",
                versions,
                Vec::new(),
                |_, route| route,
            ))
            .build()
    }

    fn selecting(strategy: nest_rs_http::ApiVersioning, default: Option<&str>) -> HttpConfig {
        HttpConfig {
            versioning: strategy,
            default_version: default.map(str::to_owned),
            ..HttpConfig::default()
        }
    }

    #[test]
    fn the_uri_strategy_reads_no_default_version() {
        let container = deployment(
            selecting(nest_rs_http::ApiVersioning::Uri, Some("9")),
            &["1"],
        );
        assert!(versioned_documents(&container).is_empty());
    }

    #[test]
    fn each_declared_version_gets_a_document_of_its_own() {
        let container = deployment(
            selecting(nest_rs_http::ApiVersioning::Header, None),
            &["1", "2"],
        );
        assert_eq!(versioned_documents(&container), ["1", "2"]);
        assert_eq!(document_path("2"), "/api-json/v2");
    }
}
