//! `GraphqlModule` — import it to serve the auto-discovered schema over HTTP.

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule};
use nestrs_http::HttpEndpointMeta;
use poem::endpoint::make_sync;
use poem::web::Html;
use poem::Route;

use crate::config::GraphqlConfig;
use crate::resolver::build_schema;

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
/// Wire it with `GraphqlModule::for_root()` (env-driven). `GraphqlConfig` loads
/// through [`ConfigModule::for_feature`] in the factory phase; the production-safe
/// [`GraphqlConfig::default`] keeps the playground + boot-time SDL emit **off**, and
/// a dev run opts them in via `.env.development` (`NESTRS_GRAPHQL__PLAYGROUND=true`,
/// `…__EMIT_SDL=true`):
///
/// ```ignore
/// #[module(imports = [GraphqlModule::for_root()])]
/// ```
pub struct GraphqlModule;

impl GraphqlModule {
    /// Env-driven: load [`GraphqlConfig`] from `NESTRS_GRAPHQL__*` (and the `.env`
    /// cascade) through the config system.
    pub fn for_root() -> GraphqlSetup {
        GraphqlSetup
    }
}

/// The configured form of [`GraphqlModule`]. Loads `GraphqlConfig` through the
/// config system in the **collect** phase, then mounts the endpoint in **register**
/// (which runs after the factory phase, so the config is available).
pub struct GraphqlSetup;

impl DynamicModule for GraphqlSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::for_feature::<GraphqlConfig>().collect(builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        let config = builder
            .snapshot()
            .get::<GraphqlConfig>()
            .expect("GraphqlConfig is loaded by ConfigModule::for_feature");
        register(builder, (*config).clone())
    }
}

/// Shared registration for both the default and configured paths.
fn register(builder: ContainerBuilder, options: GraphqlConfig) -> ContainerBuilder {
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
            // Our endpoint instead of `async_graphql_poem::GraphQL` so each
            // registered `ContextSeed` forwards per-request poem state into the
            // GraphQL context (e.g. the actor's authz `Ability`).
            let mut method = poem::post(crate::context::ContextEndpoint::new(
                schema,
                container.clone(),
            ));
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
