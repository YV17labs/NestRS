//! `GraphqlModule` — import it to serve the auto-discovered schema over HTTP.

use std::path::PathBuf;

use nestrs_core::{ContainerBuilder, DynamicModule, Module};
use nestrs_http::HttpEndpointMeta;
use poem::endpoint::make_sync;
use poem::web::Html;
use poem::Route;

use crate::resolver::build_schema;

const DEFAULT_PATH: &str = "/graphql";

/// Configuration for the GraphQL endpoint. Pass it via
/// [`GraphqlModule::for_root`] to override the defaults.
#[derive(Clone, Debug)]
pub struct GraphqlOptions {
    /// HTTP path the schema is served at (`POST` for operations, `GET` for the
    /// playground). Default `/graphql`.
    pub path: String,
    /// Serve the GraphQL playground on `GET <path>`. Default `true`.
    pub playground: bool,
    /// Where the committed SDL lives — written on boot when `emit_sdl` is
    /// `true`, ignored otherwise. Typically the app's own `schema.graphql`
    /// (`concat!(env!("CARGO_MANIFEST_DIR"), "/schema.graphql")`).
    pub schema_path: PathBuf,
    /// Whether to (re)write `schema_path` from the live schema once at boot,
    /// before serving. Drive it from the environment at the import site: `true`
    /// in development (regenerate the committed SDL as resolvers change), `false`
    /// in production (read the committed file as-is, never touch it). A write
    /// failure is logged, never fatal. Default `false`.
    pub emit_sdl: bool,
}

impl Default for GraphqlOptions {
    fn default() -> Self {
        Self {
            path: DEFAULT_PATH.into(),
            playground: true,
            schema_path: "schema.graphql".into(),
            emit_sdl: false,
        }
    }
}

/// Add to a `#[module(imports = [...])]` to expose GraphQL over HTTP:
/// `POST <path>` (queries + mutations) and, when enabled, `GET <path>` (the
/// playground).
///
/// Every `#[resolver]` in the binary self-registers via the link-time registry,
/// so the schema composes itself — there is nothing else to wire, no central
/// resolver list, no `main.rs` mount. Every `#[dataloader]` is seeded per
/// request by a schema extension built from the fully assembled container (see
/// `crate::loader`), so this module can be imported in any order relative to the
/// data modules whose services it loads.
///
/// Imported **by type** it uses [`GraphqlOptions::default`]
/// (`/graphql`, playground on):
///
/// ```ignore
/// #[module(imports = [GraphqlModule])]
/// ```
///
/// Imported **via [`for_root`](GraphqlModule::for_root)** it takes options — the
/// analog of NestJS's `GraphQLModule.forRoot(...)`:
///
/// ```ignore
/// #[module(imports = [
///     GraphqlModule::for_root(GraphqlOptions {
///         path: "/graphql".into(),
///         playground: false,
///         schema_path: "schema.graphql".into(),
///         emit_sdl: false,
///     }),
/// ])]
/// ```
pub struct GraphqlModule;

impl GraphqlModule {
    /// Configure the endpoint at the import site. Returns a [`DynamicModule`]
    /// to list in `#[module(imports = [...])]`.
    pub fn for_root(options: GraphqlOptions) -> GraphqlSetup {
        GraphqlSetup { options }
    }
}

impl Module for GraphqlModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        register(builder, GraphqlOptions::default())
    }
}

/// The configured form of [`GraphqlModule`], produced by
/// [`GraphqlModule::for_root`].
pub struct GraphqlSetup {
    options: GraphqlOptions,
}

impl DynamicModule for GraphqlSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        register(builder, self.options)
    }
}

/// Shared registration for both the default and configured paths.
fn register(builder: ContainerBuilder, options: GraphqlOptions) -> ContainerBuilder {
    let log_path = options.path.clone();
    builder.provide_meta(HttpEndpointMeta::new(
        log_path,
        "graphql",
        move |container, route: Route| {
            let schema = build_schema(container.clone());
            // This closure runs once at boot and is the only place GraphqlModule
            // holds the assembled container, so the SDL emit lives here rather
            // than in a (per-provider) lifecycle hook. Rendered from the serving
            // schema above to avoid building it twice.
            if options.emit_sdl {
                let dest = &options.schema_path;
                match std::fs::write(dest, crate::resolver::render_sdl(&schema)) {
                    Ok(()) => tracing::info!(
                        target: "nestrs::graphql",
                        "wrote GraphQL SDL to {}",
                        dest.display(),
                    ),
                    Err(err) => tracing::warn!(
                        target: "nestrs::graphql",
                        "failed to write GraphQL SDL to {}: {err}",
                        dest.display(),
                    ),
                }
            }
            let mut method = poem::post(async_graphql_poem::GraphQL::new(schema));
            if options.playground {
                let html = async_graphql::http::playground_source(
                    async_graphql::http::GraphQLPlaygroundConfig::new(options.path.as_str()),
                );
                method = method.get(make_sync(move |_| Html(html.clone())));
            }
            route.nest(options.path.as_str(), method)
        },
    ))
}
