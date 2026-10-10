//! `GraphqlModule` — import it to serve the auto-discovered schema over HTTP.

use std::any::TypeId;

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Registering};
use nest_rs_http::{DetachedWork, HttpBootCheck, HttpEndpointMeta};
use poem::Route;

use crate::config::GraphqlConfig;
use crate::context::OperationBridge;
use crate::endpoint::GetEndpoint;
use crate::resolver::{build_schema, check_operations};
use crate::subscription::SubscriptionEndpoint;

/// Mounts `POST <path>` (queries + mutations) and `GET <path>` — the graphql-ws
/// socket for subscriptions, or the playground for a plain browser request when
/// it is enabled. The schema composes itself from the resolver registry.
///
/// [`GraphqlConfig::default`] keeps the playground + boot-time SDL emit
/// **off**; a dev run opts them in via `<PREFIX>_GRAPHQL__PLAYGROUND=true` /
/// `…__EMIT_SDL=true`.
///
/// ```
/// use nest_rs_core::module;
/// use nest_rs_graphql::{GraphqlConfig, GraphqlModule};
///
/// #[module(imports = [GraphqlModule::for_root(None)])]
/// struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
/// # assert!(app.container().get::<GraphqlConfig>().is_some());
///
/// let defaults = GraphqlConfig::default();
/// assert!(!defaults.playground && !defaults.emit_sdl);
/// # Ok(())
/// # }
/// ```
pub struct GraphqlModule;

impl GraphqlModule {
    /// Pass `None` to load [`GraphqlConfig`] from `<PREFIX>_GRAPHQL__*`, or a
    /// `GraphqlConfig` to pin as the base those variables overlay, per field.
    pub fn for_root(config: impl Into<Option<GraphqlConfig>>) -> GraphqlSetup {
        GraphqlSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`GraphqlModule::for_root`]. Registers the
/// [`GraphqlConfig`] and self-mounts the `/graphql` endpoint on the HTTP transport.
pub struct GraphqlSetup {
    pinned: Option<GraphqlConfig>,
}

impl DynamicModule for GraphqlSetup {
    fn module() -> TypeId {
        TypeId::of::<GraphqlModule>()
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
            .get::<GraphqlConfig>()
            .expect("GraphqlConfig is resolved by ConfigModule::provide_feature");
        register(builder, (*config).clone())
    }
}

fn register(builder: ContainerBuilder, options: GraphqlConfig) -> ContainerBuilder {
    let log_path = options.path.clone();
    // A linked `#[resolver]` no reachable module lists is filtered out of the
    // schema; reported only when a schema mounts.
    let strict = options.strict_resolver_membership;
    let builder = builder.provide_meta(HttpBootCheck::new(move |container| {
        let unreachable = crate::resolver::unreachable_resolvers(container);
        if unreachable.is_empty() {
            return Ok(());
        }
        if strict {
            let var = nest_rs_config::var_name("graphql", "STRICT_RESOLVER_MEMBERSHIP");
            return Err(format!(
                "strict resolver-membership check failed: {unreachable:?} linked into the \
                 binary but in no reachable module. Add each to a reachable feature module's \
                 `#[module(providers = [...])]`, or clear \
                 `GraphqlConfig::strict_resolver_membership` (`{var}=false`) if the link is \
                 intentional — a workspace shipping several apps over one feature library \
                 legitimately links resolvers a given binary does not serve."
            ));
        }
        for resolver in unreachable {
            tracing::warn!(
                target: crate::TARGET,
                resolver,
                hint = "add it to a feature module's `#[module(providers = [...])]` if you meant to expose it",
                "unreachable resolver skipped from the GraphQL schema",
            );
        }
        Ok(())
    }));
    // Two contributions claiming one name fail the boot, before the mount below
    // composes a schema whose SDL and dispatch would disagree.
    //
    // async-graphql serves `_service` and `_entities` as soon as an entity
    // resolver calls `add_keys` (`schema.rs`'s `enable_federation ||
    // has_entities()`), so an `#[entity]` without `federation` is refused.
    let federation = options.federation;
    let builder = builder.provide_meta(HttpBootCheck::new(
        move |container| match check_operations(container)? {
            Some(resolver) if !federation => {
                let introspection = nest_rs_config::var_name("graphql", "DISABLE_INTROSPECTION");
                let federation_var = nest_rs_config::var_name("graphql", "FEDERATION");
                Err(format!(
                    "`{resolver}` declares an `#[entity]`, but this schema is not configured as a \
                     subgraph. An entity resolver *is* the federation surface: async-graphql \
                     serves `_service` and `_entities` the moment one exists, so the endpoint \
                     would publish its own SDL — `_service` is not covered by \
                     `{introspection}` — while the committed SDL carried the federation plumbing \
                     without the `@key` a router needs. Set `GraphqlConfig::federation = true` \
                     (or `{federation_var}=true`) and serve it behind a router, or remove the \
                     `#[entity]`."
                ))
            }
            _ => Ok(()),
        },
    ));
    // Every graphql-ws socket (outliving the HTTP connection that upgraded it) and
    // DataLoader batch (on its own task), so the transport stops them at shutdown.
    let carried = DetachedWork::new();
    let mounted = carried.clone();
    builder.provide_meta(
        HttpEndpointMeta::new(log_path, "graphql", move |container, route: Route| {
            let schema = build_schema(container.clone(), &options, &mounted);
            if options.emit_sdl {
                let dest = &options.schema_path;
                let sdl = crate::resolver::render_sdl(&schema, &options);
                match std::fs::write(dest, &sdl) {
                    Ok(()) => tracing::info!(
                        target: crate::TARGET,
                        path = %dest.display(),
                        bytes = sdl.len(),
                        "wrote GraphQL SDL"
                    ),
                    Err(err) => tracing::warn!(
                        target: crate::TARGET,
                        path = %dest.display(),
                        error = %nest_rs_core::error_message(&err),
                        "failed to write GraphQL SDL"
                    ),
                }
            }
            let bridge = Arc::new(OperationBridge::new(container.clone()));
            let schema = crate::redact::Redacted(schema);
            let method = poem::post(crate::context::ContextEndpoint::new(
                schema.clone(),
                Arc::clone(&bridge),
                options.max_batch_size,
            ))
            .get(GetEndpoint::new(
                SubscriptionEndpoint::new(schema, bridge, options.max_connection, mounted.clone()),
                options.playground.then(|| {
                    async_graphql::http::playground_source(
                        async_graphql::http::GraphQLPlaygroundConfig::new(options.path.as_str())
                            .subscription_endpoint(options.path.as_str()),
                    )
                }),
            ));
            // Guards run in-band per operation, never at the HTTP edge (`Exempt`).
            // The in-band chain reads `Public` so `AuthnGuard` admits an anonymous
            // request to the resolver gates while a present bearer is still verified.
            let method = poem::EndpointExt::data(method, ::nest_rs_http::Public);
            route.nest(options.path.as_str(), ::nest_rs_http::matched(method))
        })
        .exempt()
        .runs_detached(carried),
    )
}
