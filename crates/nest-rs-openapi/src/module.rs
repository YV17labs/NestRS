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
use nest_rs_core::{Container, ContainerBuilder, DynamicModule};
use nest_rs_http::{HttpEndpointMeta, join_path, version_path};
use poem::{Route, get};

use crate::config::OpenApiConfig;
use crate::document::{Reported, build_document, versioned_documents};
use crate::ui;

// Conventional documentation paths. The bundled Swagger UI references the spec
// and its assets *relative* to `DOCS_PATH` (see `ui.rs`), so the whole surface
// moves as one under a `global_prefix` — but the two paths must stay siblings
// (`/api` + `/api-json`, assets under `/api/`) for that relative resolution to
// hold, so they are fixed here (not yet configurable).
const DOCS_PATH: &str = "/api";
const SPEC_PATH: &str = "/api-json";
// The bundled UI's three assets, named once: the mount registers them and
// `also_mounts` declares them, and a path spelled twice is a path that can drift
// out of the declaration while still being served.
const CSS_PATH: &str = "/api/swagger-ui.css";
const BUNDLE_PATH: &str = "/api/swagger-ui-bundle.js";
const PRESET_PATH: &str = "/api/swagger-ui-standalone-preset.js";
// Every per-version document sits under `SPEC_PATH`, so one pattern covers them
// without resolving the container outside the mount closure.
const VERSIONED_SPEC_PATTERN: &str = "/api-json/*version";

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
            target: nest_rs_http::target::ROUTES,
            docs_path = DOCS_PATH,
            spec_path = SPEC_PATH,
            "openapi documentation disabled",
        );
        return builder;
    }
    builder
        // A default version nothing declares would publish an empty document,
        // which reads as "this deployment serves nothing" rather than as the
        // wiring mistake it is.
        .provide_meta(
            HttpEndpointMeta::new(DOCS_PATH, "openapi", move |container, route: Route| {
                // One ledger for every document this boot builds: a collision is a
                // property of the route table, so it is reported once rather
                // than once per document.
                let mut reported = Reported::default();
                let default = spec(container, &options, None, &mut reported);
                // Emit lives here — the only place with the assembled container.
                // The OpenAPI analogue of the GraphQL SDL emit: keep the committed
                // `openapi.json` fresh as a side effect of a dev run. Offloaded to
                // a blocking task so the synchronous write never stalls the boot
                // executor; failure still logs at `warn`. The default document is
                // what is written: a repo commits one file, and it is the one
                // `/api-json` serves.
                if options.emit_document {
                    let dest = options.document_path.clone();
                    let contents = format!("{default}\n");
                    tokio::task::spawn_blocking(move || match std::fs::write(&dest, &contents) {
                        Ok(()) => tracing::info!(
                            target: nest_rs_http::target::ROUTES,
                            path = %dest.display(),
                            bytes = contents.len(),
                            "wrote OpenAPI document",
                        ),
                        Err(err) => tracing::warn!(
                            target: nest_rs_http::target::ROUTES,
                            path = %dest.display(),
                            error = %err,
                            "failed to write OpenAPI document",
                        ),
                    });
                }
                let mut route = route
                    .at(SPEC_PATH, get(ui::spec_endpoint(default)))
                    .at(DOCS_PATH, get(ui::swagger_index))
                    .at(CSS_PATH, get(ui::swagger_css))
                    .at(BUNDLE_PATH, get(ui::swagger_bundle))
                    .at(PRESET_PATH, get(ui::swagger_preset));
                // One document per version when the version is not in the path:
                // OpenAPI keys operations by path, so two versions a header
                // selects cannot both be described at `/posts`. Swagger UI stays
                // on the default document.
                for version in versioned_documents(container) {
                    let spec = spec(container, &options, Some(&version), &mut reported);
                    route = route.at(document_path(&version), get(ui::spec_endpoint(spec)));
                }
                route
            })
            // Everything the closure above registers beyond `DOCS_PATH`,
            // declared so the version selector and the mount-path exclusivity
            // check see the whole surface rather than one corner of it.
            //
            // `/api-json` is the one that bit: it is a *sibling* of `DOCS_PATH`,
            // not a child, so nothing reasoning from `DOCS_PATH`'s subtree ever
            // covered it — and a versioned root catch-all controller silently
            // answered the document's own address. The per-version documents
            // (`/api-json/v1`) sit beneath it, so the wildcard covers them
            // without this having to resolve the container.
            .also_mounts([
                SPEC_PATH,
                VERSIONED_SPEC_PATTERN,
                CSS_PATH,
                BUNDLE_PATH,
                PRESET_PATH,
            ])
            .exempt(),
        )
}

/// The document for `claims`, serialized. Pretty-printed because it is read by
/// people as often as by generators; the compact form is the fallback for a
/// document `serde_json` cannot pretty-print, which no `Value` built here is.
fn spec(
    container: &Container,
    options: &OpenApiConfig,
    claims: Option<&str>,
    reported: &mut Reported,
) -> String {
    let document = build_document(container, options, claims, reported);
    serde_json::to_string_pretty(&document).unwrap_or_else(|_| document.to_string())
}

/// Where version `n`'s own document is served: `/api-json/v{n}`. The `/v{n}`
/// spelling comes from [`version_path`] rather than a second `format!`, so the
/// document's address and the routes it describes cannot disagree about it.
fn document_path(version: &str) -> String {
    join_path(SPEC_PATH, &version_path(Some(version), "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::DiscoveryService;
    use nest_rs_http::HttpConfig;

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

    /// A container holding one HTTP config and the controllers `versions`
    /// declares — everything the boot check reads, and nothing else.
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
        // Routing resolves the version there, so `default_version` selects
        // nothing and the document is composed from the paths as mounted.
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
