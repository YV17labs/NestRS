//! `OpenApiModule` — self-mounts `/api` (Swagger UI) + `/api-json` (the document)
//! over the HTTP transport. Both endpoints are **public** (`Posture::Public`).

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::{
    Collecting, Container, ContainerBuilder, Handler, Module, Posture, Registering,
};
use nest_rs_http::{HttpEndpointMeta, Method, declare_endpoint, join_path, version_path};

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
const VERSIONED_SPEC_PATTERN: &str = "/api-json/{*version}";

/// Add to a `#[module(imports = [...])]` to expose `GET /api-json` (the OpenAPI
/// 3.1 document) and `GET /api` (bundled Swagger UI). Wire it with
/// `OpenApiModule::for_root(None)`; configuration loads from `<PREFIX>_OPENAPI__*`.
///
/// Both endpoints are public, so they are served in a development or test
/// profile alone until `<PREFIX>_OPENAPI__ENABLED=true` (or a pinned
/// `OpenApiConfig { enabled: true, .. }`) opens them — see [`OpenApiConfig`].
/// A bare `OpenApiModule` import beside the app's `for_root` mounts nothing
/// twice.
pub struct OpenApiModule;

impl OpenApiModule {
    /// Pass `None` to load [`OpenApiConfig`] from `<PREFIX>_OPENAPI__*`, or an
    /// `OpenApiConfig` to pin as the base those variables overlay, per field.
    pub fn for_root(config: impl Into<Option<OpenApiConfig>>) -> OpenApiSetup {
        ConfigModule::setup(config.into())
    }
}

/// The configured import [`OpenApiModule::for_root`] returns.
pub type OpenApiSetup = ConfigSetup<OpenApiModule, OpenApiConfig>;

impl Module for OpenApiModule {
    // A bare import still reads `<PREFIX>_OPENAPI__*`; a `for_root(Some(cfg))`'s
    // declared value supersedes this env-only one.
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature(None::<OpenApiConfig>, builder)
    }

    // Registered once however many modules import it, so the document mounts once.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        match ConfigModule::resolved::<OpenApiConfig>(&builder.snapshot()) {
            Ok(config) => register(builder, (*config).clone()),
            Err(refusal) => builder.refuse(refusal),
        }
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
        HttpEndpointMeta::new(&DOCUMENT, DOCS_PATH, "openapi", move |container, routes| {
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
            routes
                .route(Method::GET, SPEC_PATH, ui::spec_endpoint(default))
                .route(Method::GET, DOCS_PATH, ui::swagger_index())
                .route(Method::GET, CSS_PATH, ui::swagger_css())
                .route(Method::GET, BUNDLE_PATH, ui::swagger_bundle())
                .route(Method::GET, PRESET_PATH, ui::swagger_preset())
                .route(Method::GET, INITIALIZER_PATH, ui::swagger_initializer());
            // OpenAPI keys operations by path, so a version the path does not
            // carry needs a document of its own.
            for version in versioned_documents(container) {
                let spec = spec(container, &options, Some(&version), &mut reported);
                routes.route(
                    Method::GET,
                    &document_path(&version),
                    ui::spec_endpoint(spec),
                );
            }
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
        ]),
    )
}

/// The site the document and its UI answer as: public by design, a development
/// surface no guard gates.
static DOCUMENT: Handler =
    declare_endpoint::<OpenApiModule>("OpenApiModule", DOCS_PATH, Posture::Public);

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
    use nest_rs_core::{App, Discovery, LateFactoryError, module};
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

    #[module(imports = [OpenApiModule::for_root(None)])]
    struct ServesDocumentation;

    /// A hand-written importer that imports the module in its register alone.
    struct RegistersOnly;

    impl Module for RegistersOnly {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<ServesDocumentation>()
        }
    }

    #[tokio::test]
    async fn the_module_imported_in_a_register_alone_fails_the_boot_naming_its_config() {
        let Err(refused) = App::builder().module::<RegistersOnly>().build().await else {
            panic!("the documentation would mount over a config nothing resolved");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert_eq!(late.type_name, std::any::type_name::<OpenApiConfig>());
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
            .provide_meta(nest_rs_http::__private::http_controller_meta(
                "PostsController",
                "posts",
                "/posts",
                versions,
                Vec::new(),
                |_, _| {},
            ))
            .build()
            .expect("the container builds")
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
