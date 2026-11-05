//! `GraphqlModule` — import it to serve the auto-discovered schema over HTTP.

use std::borrow::Cow;
use std::sync::Arc;

use nest_rs_config::{ConfigModule, ConfigSetup, Namespaced as _};
use nest_rs_core::__private::{
    EdgeHosts, EdgeRuntime, RequestScopes, attaches_transport, refuse_site,
};
use nest_rs_core::anyhow;
use nest_rs_core::{
    Collecting, Container, ContainerBuilder, Discovery, Edge, Module, ReachableProviders,
    Registering, Unnamed, inventory,
};
use nest_rs_http::__private::{OriginCheck, OriginRefused, checks_origin};
use nest_rs_http::{DetachedWork, HttpEndpointMeta, HttpModule, HttpRoutes, HttpTransport, Method};

use crate::config::GraphqlConfig;
use crate::context::{ENDPOINT, OperationBridge};
use crate::endpoint::GetEndpoint;
use crate::null_propagation::NullPropagated;
use crate::operation_units::OperationUnits;
use crate::redact::Redacted;
use crate::resolver::{RESOLVERS, ServedObjects, build_schema, check_operations, compose_sites};
use crate::subscription::SubscriptionEndpoint;
use crate::view::GraphqlView;

/// Mounts `POST <path>` (queries + mutations) and `GET <path>` — the graphql-ws
/// socket for subscriptions, or the playground for a plain browser request when
/// it is enabled. The schema composes itself from the resolver registry.
///
/// It imports [`HttpModule`] bare, so the endpoint is served wherever it is
/// imported. A GraphQL adapter imports `GraphqlModule` bare beside the
/// resolvers it lists, and the endpoint mounts once however many do; the app
/// imports [`GraphqlModule::for_root`] only to pin a [`GraphqlConfig`]. A
/// resolver the app reaches without it is named at `warn` when the app runs.
///
/// The bare import, and a pin over [`GraphqlConfig::default`], serve the
/// playground and answer introspection under a declared development or test
/// profile, and write the schema's SDL beside the app's manifest under
/// development; a deployment opens each by saying so.
///
/// ```
/// use nest_rs_config::Surface;
/// use nest_rs_core::module;
/// use nest_rs_graphql::{GraphqlConfig, GraphqlModule};
///
/// #[module(imports = [GraphqlModule])]
/// struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
/// # assert!(app.container().get::<GraphqlConfig>().is_some());
///
/// let defaults = GraphqlConfig::default();
/// assert_eq!(defaults.playground, Surface::ByProfile);
/// # Ok(())
/// # }
/// ```
pub struct GraphqlModule;

impl GraphqlModule {
    /// Pin `config` as the base `<PREFIX>_GRAPHQL__*` overlays, per field; the
    /// bare import reads those variables over [`GraphqlConfig`]'s defaults.
    pub fn for_root(config: GraphqlConfig) -> GraphqlSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import [`GraphqlModule::for_root`] returns.
pub type GraphqlSetup = ConfigSetup<GraphqlModule, GraphqlConfig>;

impl Module for GraphqlModule {
    // A bare import still reads `<PREFIX>_GRAPHQL__*`; a `for_root(cfg)`'s
    // declared value supersedes this env-only one.
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<Unnamed, GraphqlConfig>(builder).import::<HttpModule>()
    }

    // Registered once however many modules import it, so `/graphql` mounts once.
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        let builder = builder.import::<HttpModule>();
        match ConfigModule::resolved::<GraphqlConfig>(&builder.snapshot()) {
            Ok(config) => register(builder, (*config).clone()),
            Err(refusal) => builder.refuse(refusal),
        }
    }
}

fn register(builder: ContainerBuilder, options: GraphqlConfig) -> ContainerBuilder {
    options.warn_when_open();
    let log_path = options.path.clone();
    let builder = builder.provide_wiring("nest_rs::graphql::schema", wire_schema);
    // Every graphql-ws socket (outliving the HTTP connection that upgraded it) and
    // DataLoader batch (on its own task), so the transport stops them at shutdown.
    let carried = DetachedWork::new();
    let mounted = carried.clone();
    builder.provide_meta(checks_origin(
        HttpEndpointMeta::new(&ENDPOINT, log_path, "graphql", move |container, routes| {
            // A mount has no `Result`: a refusal fails the boot once the
            // transports are configured, and nothing is mounted meanwhile.
            if let Err(refused) = mount(container, routes, &options, &mounted) {
                refuse_site(container, refused);
            }
        })
        .carries::<GraphqlView>()
        .runs_detached(carried),
        OriginCheck::Upgrades,
        refused_origin,
    ))
}

/// Mount `POST` and `GET` on the configured path, over the schema of what the
/// app `container` holds reaches — refused for a container no app sealed.
fn mount(
    container: &Container,
    routes: &mut HttpRoutes,
    options: &GraphqlConfig,
    mounted: &DetachedWork,
) -> anyhow::Result<()> {
    let schema = build_schema(container.clone(), options, mounted)?;
    let bridge = Arc::new(OperationBridge::new(container.clone())?);
    if options.emit.writes() {
        let dest = nest_rs_core::__private::in_app_dir(&options.emit_path);
        let sdl = crate::resolver::render_sdl(&schema, options);
        match std::fs::write(&dest, &sdl) {
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
    let served = ServedObjects::of(container)?;
    crate::resolver::check_served_fields(container, &served)?;
    crate::loader::check_served_relations(container, &served)?;
    let binding = container.get_dyn::<dyn nest_rs_database::UnitOfWork>();
    let sdl = schema.sdl();
    let units = OperationUnits::new(
        NullPropagated::new(Redacted(schema), &sdl),
        binding.clone(),
        RequestScopes::new(container.clone()),
    );
    let post = crate::context::ContextEndpoint::new(
        units.clone(),
        Arc::clone(&bridge),
        options.max_batch_size,
    );
    let get = GetEndpoint::new(
        SubscriptionEndpoint::new(units, bridge, options, mounted.clone(), binding),
        options.playground.is_open().then(|| {
            async_graphql::http::playground_source(
                async_graphql::http::GraphQLPlaygroundConfig::new(options.path.as_str())
                    .subscription_endpoint(options.path.as_str()),
            )
        }),
    );
    // The transport runs the request's guards in front of both methods: the
    // endpoint's `InBand` posture lets `AuthnGuard` admit an anonymous request
    // to the operations' own gates while a presented bearer is still verified.
    routes
        .route(Method::POST, options.path.as_str(), post)
        .route(Method::GET, options.path.as_str(), get);
    Ok(())
}

/// The schema's wiring, at the kernel's wiring step: what the resolvers the app
/// reaches declare is checked, and every field's site composed, before any
/// transport is built.
fn wire_schema(container: &Container) -> anyhow::Result<()> {
    let reachable = ReachableProviders::of(container, RESOLVERS)?;
    let config = ConfigModule::resolved::<GraphqlConfig>(container)?;
    check_membership(&reachable, config.strict_resolver_membership)?;
    // Two contributions claiming one name fail the boot, before the mount
    // composes a schema whose SDL and dispatch would disagree.
    //
    // async-graphql serves `_service` and `_entities` as soon as a type is
    // keyed (`schema.rs`'s `enable_federation || has_entities()`), so a keyed
    // schema without `federation` is refused.
    if let Some(resolver) = check_operations(&reachable).map_err(anyhow::Error::msg)?
        && !config.federation
    {
        let introspection = GraphqlConfig::INTROSPECTION.var(GraphqlConfig::NAMESPACE);
        let federation = GraphqlConfig::FEDERATION;
        let federation_var = federation.var(GraphqlConfig::NAMESPACE);
        let federation_field = federation.path();
        anyhow::bail!(
            "`{resolver}` keys an entity — an `#[entity]`, or an object it returns carrying \
             `#[graphql(entity)]` — but this schema is not configured as a subgraph. An \
             entity resolver *is* the federation surface: async-graphql serves `_service` and \
             `_entities` the moment one exists, so the endpoint would publish its own SDL — \
             `_service` is not covered by `{introspection}` — while the committed SDL carried \
             the federation plumbing without the `@key` a router needs. Set \
             `{federation_field} = true` (or `{federation_var}=true`) and serve it \
             behind a router, or remove the key."
        );
    }
    compose_sites(container, &reachable)?;
    crate::federation::compose_sites(container)
}

/// A linked `#[resolver]` no reachable module lists is filtered out of the
/// schema: a boot failure when `strict`, a `warn` each otherwise.
fn check_membership(reachable: &ReachableProviders, strict: bool) -> anyhow::Result<()> {
    let unreachable = crate::resolver::unreachable_resolvers(reachable);
    if unreachable.is_empty() {
        return Ok(());
    }
    if strict {
        let strict = GraphqlConfig::STRICT_RESOLVER_MEMBERSHIP;
        let (field, var) = (strict.path(), strict.var(GraphqlConfig::NAMESPACE));
        anyhow::bail!(
            "strict resolver-membership check failed: {unreachable:?} linked into the binary but \
             in no reachable module. Add each to a reachable feature module's \
             `#[module(providers = [...])]`, or clear `{field}` (`{var}=false`) if the link is \
             intentional — a workspace shipping several apps over one feature library \
             legitimately links resolvers a given binary does not serve."
        );
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
}

nest_rs_core::inventory::submit! {
    EdgeRuntime {
        edge: Edge::Graphql,
        import: "nest_rs::graphql::GraphqlModule",
        attached: serves_graphql,
        hosts: resolver_hosts,
    }
}

/// Whether the app `container` was booted for mounts `/graphql` on a served
/// HTTP transport.
fn serves_graphql(container: &Container) -> bool {
    attaches_transport::<HttpTransport>(container)
        && Discovery::new(container)
            .meta::<HttpEndpointMeta>()
            .iter()
            .any(|endpoint| std::ptr::eq(endpoint.meta.handler(), &ENDPOINT))
}

/// Every `#[resolver]` this binary links.
fn resolver_hosts(_: &Container) -> EdgeHosts {
    inventory::iter::<crate::resolver::ResolverDescriptor>()
        .map(|resolver| ((resolver.resolver)(), Cow::Borrowed(resolver.name)))
        .collect()
}

/// A graphql-ws upgrade from a browser origin the transport's policy refuses,
/// filed on this edge's target.
fn refused_origin(refused: &OriginRefused) {
    nest_rs_http::__origin_refused!(target: crate::TARGET, refused);
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{App, LateFactoryError, module};

    use super::*;

    #[module(imports = [GraphqlModule])]
    struct ServesGraphql;

    /// A hand-written importer that imports the module in its register alone.
    struct RegistersOnly;

    impl Module for RegistersOnly {
        fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
            builder.import::<ServesGraphql>()
        }
    }

    #[test]
    fn the_schema_wired_over_a_container_no_app_sealed_is_refused_naming_its_reader() {
        let by_hand = Container::builder()
            .provide(GraphqlConfig::default())
            .build()
            .expect("the container builds");

        let Err(refused) = wire_schema(&by_hand) else {
            panic!("an ungated discovery is refused, never served whole");
        };

        let ungated = refused
            .downcast_ref::<nest_rs_core::UngatedDiscoveryError>()
            .unwrap_or_else(|| panic!("not the ungated refusal: {refused:#}"));
        assert_eq!(ungated.reader, "GraphQL resolver");
    }

    #[tokio::test]
    async fn the_module_imported_in_a_register_alone_fails_the_boot_naming_its_config() {
        let Err(refused) = App::builder().module::<RegistersOnly>().build().await else {
            panic!("the schema would mount over a config nothing resolved");
        };
        let late = refused
            .downcast_ref::<LateFactoryError>()
            .unwrap_or_else(|| panic!("not the late-factory refusal: {refused:#}"));
        assert_eq!(late.type_name, std::any::type_name::<GraphqlConfig>());
    }
}
