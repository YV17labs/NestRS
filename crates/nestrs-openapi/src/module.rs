//! `OpenApiModule` — import it to serve the auto-generated OpenAPI document and
//! Swagger UI over HTTP.

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule};
use nestrs_http::HttpEndpointMeta;
use poem::{get, Route};

use crate::config::OpenApiConfig;
use crate::document::build_document;
use crate::ui;

// NestJS convention (`SwaggerModule.setup('api', …)`): UI at `/api`, document
// at `/api-json`. The OpenAPI spec mandates no serving path, so we follow the
// reference NestJS surface this framework mirrors. The bundled Swagger UI
// references these paths absolutely, so they are fixed (not yet configurable).
const DOCS_PATH: &str = "/api";
const SPEC_PATH: &str = "/api-json";

/// Add to a `#[module(imports = [...])]` to expose:
/// - `GET /api-json` — the OpenAPI 3.1 document, and
/// - `GET /api` — bundled Swagger UI.
///
/// Like [`nestrs_graphql::GraphqlModule`], it self-mounts via an
/// [`HttpEndpointMeta`]: there is nothing to wire in `main.rs`. The spec is
/// composed from every `#[controller]` linked into the binary, so importing
/// this module is the only step.
///
/// Wire it with `OpenApiModule::for_root()` (env-driven). The document metadata
/// loads through [`ConfigModule::for_feature`] from `NESTRS_OPENAPI__*`:
///
/// ```ignore
/// #[module(imports = [OpenApiModule::for_root()])]
/// ```
pub struct OpenApiModule;

impl OpenApiModule {
    /// Env-driven: load [`OpenApiConfig`] from `NESTRS_OPENAPI__*` (and the `.env`
    /// cascade) through the config system.
    pub fn for_root() -> OpenApiSetup {
        OpenApiSetup
    }
}

/// The configured form of [`OpenApiModule`]. Loads `OpenApiConfig` through the
/// config system in **collect**, then installs the endpoint in **register** (after
/// the factory phase, so the config is available).
pub struct OpenApiSetup;

impl DynamicModule for OpenApiSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::for_feature::<OpenApiConfig>().collect(builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        let config = builder
            .snapshot()
            .get::<OpenApiConfig>()
            .expect("OpenApiConfig is loaded by ConfigModule::for_feature");
        register(builder, (*config).clone())
    }
}

/// Shared registration for both the default and configured paths: install the
/// self-mounting endpoint, capturing the document metadata in the mount closure
/// (the document itself is built once at `configure`, when every controller is
/// present).
fn register(builder: ContainerBuilder, options: OpenApiConfig) -> ContainerBuilder {
    builder.provide_meta(HttpEndpointMeta::new(
        DOCS_PATH,
        "openapi",
        move |container, route: Route| {
            let document = build_document(
                container,
                &options.title,
                &options.version,
                options.description.as_deref(),
            );
            let spec =
                serde_json::to_string_pretty(&document).unwrap_or_else(|_| document.to_string());
            route
                .at(SPEC_PATH, get(ui::spec_endpoint(spec)))
                .at(DOCS_PATH, get(ui::swagger_index))
                .at("/api/swagger-ui.css", get(ui::swagger_css))
                .at("/api/swagger-ui-bundle.js", get(ui::swagger_bundle))
                .at(
                    "/api/swagger-ui-standalone-preset.js",
                    get(ui::swagger_preset),
                )
        },
    ))
}
