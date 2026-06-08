//! `GraphqlModule` — import it to serve the auto-discovered schema over HTTP.

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use nest_rs_http::HttpEndpointMeta;
use poem::Route;
use poem::endpoint::make_sync;
use poem::web::Html;

use crate::config::GraphqlConfig;
use crate::resolver::build_schema;

/// Mounts `POST <path>` (queries + mutations) and, when enabled, `GET <path>`
/// (the playground). The schema composes itself from the resolver registry;
/// dataloaders are seeded per request by an extension built from the
/// assembled container, so this module's import order is irrelevant.
///
/// [`GraphqlConfig::default`] keeps the playground + boot-time SDL emit
/// **off** for production safety; a dev run opts them in via
/// `NESTRS_GRAPHQL__PLAYGROUND=true` / `…__EMIT_SDL=true`.
///
/// ```ignore
/// #[module(imports = [GraphqlModule::for_root()])]
/// ```
pub struct GraphqlModule;

impl GraphqlModule {
    /// Pass `None` to load [`GraphqlConfig`] from `NESTRS_GRAPHQL__*`, or a
    /// `GraphqlConfig` to pin it (wins over the environment).
    pub fn for_root(config: impl Into<Option<GraphqlConfig>>) -> GraphqlSetup {
        GraphqlSetup {
            pinned: config.into(),
        }
    }
}

pub struct GraphqlSetup {
    pinned: Option<GraphqlConfig>,
}

impl DynamicModule for GraphqlSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        let config = builder
            .snapshot()
            .get::<GraphqlConfig>()
            .expect("GraphqlConfig is resolved by ConfigModule::provide_feature");
        register(builder, (*config).clone())
    }
}

fn register(builder: ContainerBuilder, options: GraphqlConfig) -> ContainerBuilder {
    let log_path = options.path.clone();
    // Marks the schema as composed in this app so the boot runs the
    // resolver-membership check (skipped when resolvers link but no schema
    // mounts).
    let builder = builder.provide(nest_rs_core::ResolverSchemaActive);
    builder.provide_meta(HttpEndpointMeta::new(
        log_path,
        "graphql",
        move |container, route: Route| {
            let schema = build_schema(container.clone());
            // SDL emit lives here — this is the only place we hold the
            // assembled container; rendered from the serving schema to avoid
            // building it twice.
            if options.emit_sdl {
                let dest = &options.schema_path;
                match std::fs::write(dest, crate::resolver::render_sdl(&schema)) {
                    Ok(()) => tracing::info!(
                        target: "nest_rs::graphql",
                        "wrote GraphQL SDL to {}",
                        dest.display(),
                    ),
                    Err(err) => tracing::warn!(
                        target: "nest_rs::graphql",
                        "failed to write GraphQL SDL to {}: {err}",
                        dest.display(),
                    ),
                }
            }
            // Our endpoint instead of `async_graphql_poem::GraphQL` so each
            // `GraphqlContextSeed` forwards per-request poem state into the context.
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
