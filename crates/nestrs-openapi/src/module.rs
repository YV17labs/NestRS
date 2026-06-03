//! `OpenApiModule` — self-mounts `/api` (Swagger UI) + `/api-json` (the document)
//! over the HTTP transport. Import it; no `main.rs` wiring.

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule};
use nestrs_http::HttpEndpointMeta;
use poem::{get, Route};

use crate::config::OpenApiConfig;
use crate::document::build_document;
use crate::ui;

// Conventional documentation paths. The bundled Swagger UI references these
// absolutely, so they are fixed (not yet configurable).
const DOCS_PATH: &str = "/api";
const SPEC_PATH: &str = "/api-json";

/// Add to a `#[module(imports = [...])]` to expose `GET /api-json` (the OpenAPI
/// 3.1 document) and `GET /api` (bundled Swagger UI). Wire it with
/// `OpenApiModule::for_root()`; configuration loads from `NESTRS_OPENAPI__*`.
pub struct OpenApiModule;

impl OpenApiModule {
    /// Pass `None` to load [`OpenApiConfig`] from `NESTRS_OPENAPI__*`, or an
    /// `OpenApiConfig` to pin it in code (wins over the environment).
    pub fn for_root(config: impl Into<Option<OpenApiConfig>>) -> OpenApiSetup {
        OpenApiSetup {
            pinned: config.into(),
        }
    }
}

pub struct OpenApiSetup {
    pinned: Option<OpenApiConfig>,
}

impl DynamicModule for OpenApiSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        let config = builder
            .snapshot()
            .get::<OpenApiConfig>()
            .expect("OpenApiConfig is resolved by ConfigModule::provide_feature");
        register(builder, (*config).clone())
    }
}

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
