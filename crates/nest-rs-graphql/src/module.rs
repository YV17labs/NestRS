//! `GraphqlModule` — import it to serve the auto-discovered schema over HTTP.

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use nest_rs_http::{HttpBootCheck, HttpEndpointMeta};
use poem::http::header;
use poem::{Endpoint, IntoResponse, Request, Response, Route};

use crate::config::GraphqlConfig;
use crate::context::OperationBridge;
use crate::resolver::{build_schema, check_operations};
use crate::subscription::SubscriptionEndpoint;

/// Mounts `POST <path>` (queries + mutations) and `GET <path>` — the graphql-ws
/// socket for subscriptions, or the playground for a plain browser request when
/// it is enabled. The schema composes itself from the resolver registry;
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
    // Merging is what this transport does, so the one failure mode merging adds
    // — two contributions claiming one addressable name — is a boot error here,
    // as it already is on HTTP and MCP. It runs at `configure`, before the
    // mount below composes a schema whose SDL and dispatch would disagree.
    //
    // The same pass answers whether anything declared an `#[entity]`, which the
    // refusal below is the whole reader of — one walk of the resolver inventory
    // and one scratch registry per boot, rather than two of each and two copies
    // of the reachability filter to keep in step.
    //
    // `federation = false` has to *mean* something, and on its own it does not:
    // async-graphql creates `_service` and `_entities` as soon as any entity
    // resolver has called `add_keys`, whatever the builder was told
    // (`schema.rs`'s `enable_federation || has_entities()`). So a single
    // `#[entity]` publishes the schema's own SDL to anyone who can reach the
    // endpoint — introspection setting notwithstanding, since `_service` is
    // outside that gate — and the flag would be a comment. The boot refuses the
    // combination instead: declaring an entity is declaring a subgraph, and a
    // subgraph is a deployment decision (it belongs behind a router), so the
    // developer says both or neither.
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
    builder.provide_meta(
        HttpEndpointMeta::new(log_path, "graphql", move |container, route: Route| {
            let schema = build_schema(container.clone(), &options);
            // SDL emit lives here — this is the only place we hold the
            // assembled container; rendered from the serving schema to avoid
            // building it twice.
            if options.emit_sdl {
                let dest = &options.schema_path;
                let sdl = crate::resolver::render_sdl(&schema, &options);
                match std::fs::write(dest, &sdl) {
                    Ok(()) => tracing::info!(
                        target: "nest_rs::graphql",
                        path = %dest.display(),
                        bytes = sdl.len(),
                        "wrote GraphQL SDL"
                    ),
                    Err(err) => tracing::warn!(
                        target: "nest_rs::graphql",
                        path = %dest.display(),
                        error = %err,
                        "failed to write GraphQL SDL"
                    ),
                }
            }
            // Which guard gates operations and which seeds fire is resolved
            // once here and shared by both endpoints — the POST path and the
            // socket must not be able to disagree about who is authenticated.
            let bridge = Arc::new(OperationBridge::new(container.clone()));
            // Our endpoint instead of `async_graphql_poem::GraphQL` so each
            // `GraphqlContextSeed` forwards per-request poem state into the context.
            let method = poem::post(crate::context::ContextEndpoint::new(
                schema.clone(),
                Arc::clone(&bridge),
                options.max_batch_size,
            ))
            .get(GetEndpoint {
                subscriptions: SubscriptionEndpoint::new(schema, bridge, options.max_connection),
                playground: options.playground.then(|| {
                    async_graphql::http::playground_source(
                        async_graphql::http::GraphQLPlaygroundConfig::new(options.path.as_str())
                            .subscription_endpoint(options.path.as_str()),
                    )
                }),
            });
            // GraphQL authenticates per-operation — through the registered
            // `GraphqlOperationGuard` bridge, or the global-pool fallback when
            // none is registered — never at the HTTP edge (the self-mount is
            // `Exempt` below, so guards run exactly once, in-band). The
            // `Public` marker is load-bearing: the in-band chain reads it so
            // an `AuthnGuard` admits an anonymous request through to the
            // resolver gates (GraphQL errors in a 200, not a blanket HTTP
            // 401) while a present bearer is still verified.
            let method = poem::EndpointExt::data(method, ::nest_rs_core::Public);
            route.nest(options.path.as_str(), method)
        })
        .exempt(),
    )
}

/// `GET <path>`: the graphql-ws socket, or the playground.
///
/// One path rather than two, because that is what a graphql-ws client assumes —
/// `graphql-ws`, Apollo and the playground all point their socket at the same
/// URL they POST to. The two are told apart by the request, not by the route:
/// an upgrade is a subscription, anything else is a browser.
struct GetEndpoint<E> {
    subscriptions: SubscriptionEndpoint<E>,
    /// Rendered once at mount; `None` when the playground is off (the default).
    playground: Option<String>,
}

impl<E: async_graphql::Executor> Endpoint for GetEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        // `Connection: Upgrade` is the hop-by-hop header a proxy may rewrite or
        // list beside other tokens, so the *presence of an upgrade target*
        // (`Upgrade: websocket`) is what decides — the same fact poem's own
        // `WebSocket` extractor requires, checked before we take the socket path
        // so a plain browser still gets the playground.
        let upgrading = req
            .headers()
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
        if upgrading {
            return self.subscriptions.call(req).await;
        }
        match &self.playground {
            Some(html) => Ok(poem::web::Html(html.clone()).into_response()),
            // No playground and not an upgrade: the path serves POST and
            // sockets, so a bare GET is the wrong method, not a missing route.
            None => Err(poem::Error::from_status(
                poem::http::StatusCode::METHOD_NOT_ALLOWED,
            )),
        }
    }
}
