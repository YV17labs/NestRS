//! `OpenApiModule` — self-mounts `/api` (Swagger UI) + `/api-json` (the document)
//! over the HTTP transport. Import it; no `main.rs` wiring.
//!
//! Both endpoints are **public** (`EdgePosture::Exempt`) — an enabled document is
//! served to anyone. Gate it with [`OpenApiConfig::enabled`](crate::OpenApiConfig)
//! (default `true` for local ergonomics): **production deployments should set
//! `NESTRS_OPENAPI__ENABLED=false`** (or pin `OpenApiConfig { enabled: false, .. }`,
//! or only import the module on an internal-facing app) so the schema is not
//! published publicly. When disabled the module mounts neither endpoint and logs
//! one boot event, so an imported-but-off module is never silently inert.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use nest_rs_http::HttpEndpointMeta;
use poem::{Route, get};

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
///
/// Both endpoints are public; set `NESTRS_OPENAPI__ENABLED=false` (or pin
/// `OpenApiConfig { enabled: false, .. }`) to mount neither — see
/// [`OpenApiConfig`].
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

/// The configured import produced by [`OpenApiModule::for_root`]. Registers the
/// [`OpenApiConfig`] and self-mounts the `/api-json` + `/api` endpoints.
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
    // Disabled ⇒ mount neither endpoint (fail-secure for production, where the
    // public document should not be exposed). Not a failure — an explicit,
    // documented opt-out — so emit a boot event (never silently inert) and skip
    // the self-mount by returning the builder untouched.
    if !options.enabled {
        tracing::info!(
            target: "nest_rs::routes",
            docs_path = DOCS_PATH,
            spec_path = SPEC_PATH,
            "openapi documentation disabled",
        );
        return builder;
    }
    builder.provide_meta(
        HttpEndpointMeta::new(DOCS_PATH, "openapi", move |container, route: Route| {
            let document = build_document(
                container,
                &options.title,
                &options.version,
                options.description.as_deref(),
            );
            let spec =
                serde_json::to_string_pretty(&document).unwrap_or_else(|_| document.to_string());
            // Emit lives here — the only place with the assembled container.
            // The OpenAPI analogue of the GraphQL SDL emit: keep the committed
            // `openapi.json` fresh as a side effect of a dev run. Offloaded to
            // a blocking task so the synchronous write never stalls the boot
            // executor; failure still logs at `warn`.
            if options.emit_document {
                let dest = options.document_path.clone();
                let contents = format!("{spec}\n");
                tokio::task::spawn_blocking(move || match std::fs::write(&dest, &contents) {
                    Ok(()) => tracing::info!(
                        target: "nest_rs::routes",
                        path = %dest.display(),
                        bytes = contents.len(),
                        "wrote OpenAPI document",
                    ),
                    Err(err) => tracing::warn!(
                        target: "nest_rs::routes",
                        path = %dest.display(),
                        error = %err,
                        "failed to write OpenAPI document",
                    ),
                });
            }
            route
                .at(SPEC_PATH, get(ui::spec_endpoint(spec)))
                .at(DOCS_PATH, get(ui::swagger_index))
                .at("/api/swagger-ui.css", get(ui::swagger_css))
                .at("/api/swagger-ui-bundle.js", get(ui::swagger_bundle))
                .at(
                    "/api/swagger-ui-standalone-preset.js",
                    get(ui::swagger_preset),
                )
        })
        .exempt(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::DiscoveryService;

    // Count the self-mount edges `register` provided for the given `enabled`
    // config. The disabled path must contribute zero — no public schema surface.
    fn mount_count(enabled: bool) -> usize {
        let builder = register(
            ContainerBuilder::default(),
            OpenApiConfig {
                enabled,
                ..OpenApiConfig::default()
            },
        );
        DiscoveryService::new(&builder.snapshot())
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
}
