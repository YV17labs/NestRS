//! What runs the two federation root fields — each its own site on the
//! pipeline — and the ceiling on how many references one call may carry.
//!
//! `_service` and `_entities` are resolved by async-graphql's own `QueryRoot`,
//! *above* [`DiscoveredQuery`](crate::resolver::DiscoveredQuery), so no
//! resolver's wrapper sees them. A schema [`Extension`] is the one seam in
//! front of a root field: the field's unit runs on `resolve`, the ceiling on
//! `parse_query`.
//!
//! A federation field belongs to no resolver. `_entities`' guard stage is the
//! app-wide pool alone, which an `#[entity]`'s own site therefore stops
//! composing; `_service` states no posture and has no member stating one, so its
//! stage adds what else the `/graphql` request ran that identifies a caller.

use std::any::TypeId;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextParseQuery, NextPrepareRequest, NextResolve,
    ResolveInfo,
};
use async_graphql::parser::types::{ExecutableDocument, OperationType, Selection, SelectionSet};
use async_graphql::{
    Error, Name, Positioned, Request, ServerError, ServerResult, Value, Variables,
};
use nest_rs_config::Namespaced as _;
use nest_rs_core::__private::{
    GuardStage, GuardStageFactory, HandlerDeclaration, MetaLevels, Site, SiteDeclaration,
    SiteLayers, declare_handler, declare_site,
};
use nest_rs_core::anyhow;
use nest_rs_core::{Container, Handler, Posture};
use nest_rs_guards::__private::{global_guards_active, identifies, resolve_global_chain};
use nest_rs_http::__private::carried_request_chain;

use crate::config::GraphqlConfig;
use crate::error::{FieldFailure, Unserved, opaque_error};
use crate::operation::{
    DENIED_FEDERATION_FIELD, FieldGuardStage, FieldUnit, ROOT_FIELD, in_field_unit,
};
use crate::view::{FieldRole, GraphqlView};

/// The `Query`-root field a router calls to read a subgraph's SDL.
pub(crate) const SERVICE_FIELD: &str = "_service";
/// The `Query`-root field a router calls with entity references.
pub(crate) const ENTITIES_FIELD: &str = "_entities";
/// Its one argument, whose length is the fan-out.
const REPRESENTATIONS_ARG: &str = "representations";

/// Whether the federation root fields run the app-wide guard pool: whenever
/// the app declares one.
pub(crate) fn gate_runs(container: &Container) -> bool {
    global_guards_active(container)
}

/// The handler `_service` runs as.
static SERVICE: Handler = federation_field(SERVICE_FIELD);
/// The handler `_entities` runs as.
static ENTITIES: Handler = federation_field(ENTITIES_FIELD);

/// The site `_service` runs.
static SERVICE_SITE: Site = declare_site(&SERVICE, federation_field_site(service_guard_stage));
/// The site `_entities` runs.
static ENTITIES_SITE: Site = declare_site(&ENTITIES, federation_field_site(entities_guard_stage));

/// How a boot error names the site `_service` runs.
const SERVICE_SITE_LABEL: &str = "POST /graphql (_service)";

/// A federation root field's declaration: a root field no resolver declares,
/// gated by `guard_stage` alone, inside the app's filters and interceptors.
const fn federation_field_site(guard_stage: GuardStageFactory) -> SiteDeclaration {
    SiteDeclaration {
        layers: SiteLayers::default,
        global: ROOT_FIELD,
        guard_stage: Some(guard_stage),
        class_gate: None,
    }
}

/// A federation root field's handler: a field no resolver declares.
const fn federation_field(field: &'static str) -> Handler {
    declare_handler(HandlerDeclaration {
        unit: crate::unit::FIELD,
        host: TypeId::of::<FederationExtension>(),
        host_name: "FederationExtension",
        method: "",
        address: field,
        posture: Posture::Gated,
        meta: MetaLevels::default,
    })
}

/// The pool, as on every unit, and each other guard the `/graphql` request ran that
/// identifies a caller: `_service` answers the whole SDL and no member's posture stands
/// behind it, so one of them admitting the anonymous request is the one thing left to
/// refuse it. A guard identifying no one decided at the request all it decides.
fn service_guard_stage(
    container: &Container,
    _: &Handler,
) -> anyhow::Result<Option<Arc<dyn GuardStage>>> {
    let pool = resolve_global_chain(container, SERVICE_SITE_LABEL);
    let mut chain = carried_request_chain::<GraphqlView>(container, SERVICE_SITE_LABEL)?;
    chain.retain(|guard| {
        identifies(guard) || pool.iter().any(|pooled| pooled.type_id == guard.type_id)
    });
    Ok(FieldGuardStage::of(chain, DENIED_FEDERATION_FIELD))
}

/// The app-wide pool, the whole chain of a field that belongs to no resolver;
/// none when the app lists no pool, so an `#[entity]` folds it itself.
fn entities_guard_stage(
    container: &Container,
    _: &Handler,
) -> anyhow::Result<Option<Arc<dyn GuardStage>>> {
    if !gate_runs(container) {
        return Ok(None);
    }
    let chain = resolve_global_chain(container, "POST /graphql (_entities)");
    Ok(FieldGuardStage::of(chain, DENIED_FEDERATION_FIELD))
}

/// What the boot says when a subgraph whose guards identify no caller serves its
/// SDL in a deployment.
const SDL_OPEN: &str =
    "GraphQL subgraph SDL is served to callers no guard identified outside a dev profile";

/// Refuse a subgraph `_service` would answer to whoever reaches it: no guard its
/// `/graphql` request runs identifies or authorizes the caller, and the configuration
/// does not open its SDL. An opening taking effect in a deployment is said at `warn`.
pub(crate) fn check_sdl_exposure(
    container: &Container,
    config: &GraphqlConfig,
) -> anyhow::Result<()> {
    if !config.federation {
        return Ok(());
    }
    let pool = resolve_global_chain(container, SERVICE_SITE_LABEL);
    let carried = carried_request_chain::<GraphqlView>(container, SERVICE_SITE_LABEL)?;
    if pool.iter().chain(&carried).any(identifies) {
        return Ok(());
    }
    if config.federation_sdl.is_open() {
        if let Some(environment) = config.federation_sdl.opened_in_deployment() {
            tracing::warn!(
                target: crate::TARGET,
                environment = environment.as_str(),
                "{SDL_OPEN}",
            );
        }
        return Ok(());
    }
    let opening = GraphqlConfig::FEDERATION_SDL;
    let (field, var) = (opening.path(), opening.var(GraphqlConfig::NAMESPACE));
    anyhow::bail!(
        "this schema serves as a subgraph, and no guard its `/graphql` request runs \
         identifies a caller, so `_service` would answer its whole SDL to whoever reaches \
         the endpoint. List a guard identifying the router — an authentication or \
         authorization guard in the app's `GlobalGuards`, or in an \
         `EndpointGuards<GraphqlView, …>` — or, for a subgraph only its router reaches on a \
         private network, open it: `{field}: Surface::Open` (`{var}=true`), or \
         `Surface::ByProfile` under a development or test profile alone."
    )
}

/// Compose both federation sites against `container`, so a layer they fold
/// that does not resolve refuses the boot.
pub(crate) fn compose_sites(container: &Container) -> anyhow::Result<()> {
    SERVICE_SITE.chains(container)?;
    ENTITIES_SITE.chains(container)?;
    Ok(())
}

/// Installs [`FederationExtension`] on the served schema.
pub(crate) struct FederationExtensionFactory {
    max_representations: Option<usize>,
}

impl FederationExtensionFactory {
    /// `None` when the schema is no subgraph and the app lists no guard pool:
    /// an idle extension costs every field a boxed indirection. A pool installs
    /// it whatever `federation` says, so `_entities` runs the pool wherever an
    /// `#[entity]`'s own site skips it ([`gate_runs`]); outside a subgraph the
    /// boot refuses a keyed schema, the one async-graphql would serve the
    /// federation fields from.
    pub(crate) fn for_schema(container: &Container, config: &GraphqlConfig) -> Option<Self> {
        (config.federation || gate_runs(container)).then_some(Self {
            max_representations: config.max_representations,
        })
    }
}

impl ExtensionFactory for FederationExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(FederationExtension {
            max_representations: self.max_representations,
            operation_name: Mutex::new(None),
        })
    }
}

struct FederationExtension {
    max_representations: Option<usize>,
    /// The operation this request selected, captured in `prepare_request` — the
    /// one hook that carries it — so the ceiling can be charged to the operation
    /// that will actually run. An extension instance is built per request
    /// (`ExtensionFactory::create`), so this holds one request's answer.
    operation_name: Mutex<Option<String>>,
}

/// The error a federation field answered itself, carried through its unit as
/// it was built: its path and locations are async-graphql's to keep.
struct Answered(ServerError);

#[async_trait]
impl Extension for FederationExtension {
    async fn prepare_request(
        &self,
        ctx: &ExtensionContext<'_>,
        request: Request,
        next: NextPrepareRequest<'_>,
    ) -> ServerResult<Request> {
        let request = next.run(ctx, request).await?;
        // A poisoned lock must not deny service: the value is advisory.
        *self
            .operation_name
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = request.operation_name.clone();
        Ok(request)
    }

    async fn parse_query(
        &self,
        ctx: &ExtensionContext<'_>,
        query: &str,
        variables: &Variables,
        next: NextParseQuery<'_>,
    ) -> ServerResult<ExecutableDocument> {
        let document = next.run(ctx, query, variables).await?;
        if let Some(max) = self.max_representations {
            let selected = self
                .operation_name
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            refuse_oversized_entity_calls(&document, variables, selected.as_deref(), max)?;
        }
        Ok(document)
    }

    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<Value>> {
        // A nested `_entities` is an app's own field, with its own chain.
        let site = match (info.parent_type, info.name) {
            ("Query", SERVICE_FIELD) => &SERVICE_SITE,
            ("Query", ENTITIES_FIELD) => &ENTITIES_SITE,
            _ => return next.run(ctx, info).await,
        };
        let position = info.field.name.pos;
        let field = info.name;
        // Fails closed: without the container there is no site to run it on.
        let Some(container) = ctx.data_opt::<Container>() else {
            let unserved = Unserved {
                field: site.handler().address(),
            };
            return Err(opaque_error(&unserved).into_server_error(position));
        };
        let unit = FieldUnit::new(site, container, OperationType::Query, FieldRole::Federation);
        let tail = move || async move {
            next.run(ctx, info).await.map_err(|answered| {
                let carried = Error {
                    message: answered.message.clone(),
                    source: Some(Arc::new(Answered(answered))),
                    extensions: None,
                };
                FieldFailure::from(carried)
            })
        };
        in_field_unit(
            site.handler(),
            OperationType::Query,
            "federation",
            field,
            async move {
                unit.run(tail).await.map_err(|error| {
                    match error
                        .source
                        .as_ref()
                        .and_then(|source| source.downcast_ref::<Answered>())
                    {
                        Some(Answered(answered)) => answered.clone(),
                        None => error.into_server_error(position),
                    }
                })
            },
        )
        .await
    }
}

/// Refuse the selected operation when its `_entities` calls carry more
/// references, **in total** across aliases, than `max`.
///
/// Read off the parsed document: by the time `find_entity` runs, async-graphql
/// has launched one future per element. A `_entities` under `@skip(if: true)`
/// is still counted, the safe direction.
fn refuse_oversized_entity_calls(
    document: &ExecutableDocument,
    variables: &Variables,
    operation_name: Option<&str>,
    max: usize,
) -> ServerResult<()> {
    let selected = match operation_name {
        Some(name) => document
            .operations
            .iter()
            .find(|(key, _)| key.map(|k| k.as_str()) == Some(name))
            .map(|(_, operation)| operation),
        // Unnamed: async-graphql runs the single operation; several without a
        // name is an error of its own.
        None => document.operations.iter().next().map(|(_, op)| op),
    };
    let Some(operation) = selected else {
        return Ok(());
    };
    if operation.node.ty != OperationType::Query {
        return Ok(());
    }

    let mut total = 0usize;
    let mut position = None;
    let mut seen = HashSet::new();
    visit_root_fields(
        document,
        &operation.node.selection_set,
        &mut seen,
        &mut |field| {
            if field.node.name.node != ENTITIES_FIELD {
                return Ok(());
            }
            total = total.saturating_add(representation_count(field, variables));
            position.get_or_insert(field.pos);
            Ok(())
        },
    )?;

    if total <= max {
        return Ok(());
    }
    let setting = GraphqlConfig::MAX_REPRESENTATIONS;
    let (field, key) = (
        setting.field().path(),
        setting.var(GraphqlConfig::NAMESPACE),
    );
    Err(ServerError::new(
        format!(
            "this operation carries {total} `_entities` representations, over this \
             schema's ceiling of {max}. Each one is resolved on its own — body, \
             posture gate and mask — so the total list length is the fan-out, \
             however many `_entities` fields it is spread across. Split the call, \
             or raise `{field}` (`{key}`; `0` ⇒ unlimited).",
        ),
        position,
    ))
}

/// How many references one `_entities` field carries; a router's bare
/// `$representations` takes the cheap path.
///
/// `into_const_with` rather than a match on the AST value: async-graphql
/// re-exports only the *const* `Value`, and naming the other would add a second
/// dependency pinned in lockstep.
fn representation_count(
    field: &Positioned<async_graphql::parser::types::Field>,
    variables: &Variables,
) -> usize {
    let Some((_, value)) = field
        .node
        .arguments
        .iter()
        .find(|(name, _)| name.node == REPRESENTATIONS_ARG)
    else {
        return 0;
    };
    // A non-list is async-graphql's coercion error to report, not a ceiling breach.
    let len_of = |value: &Value| match value {
        Value::List(items) => Some(items.len()),
        _ => None,
    };
    let owned = value.node.clone();
    match owned.clone().into_const_with::<Name>(Err) {
        Ok(resolved) => len_of(&resolved).unwrap_or(0),
        Err(name) => match variables.get(&name).and_then(len_of) {
            Some(len) => len,
            // `[$a, $b]`: only a full resolve counts it.
            None => owned
                .into_const_with::<()>(|name| variables.get(&name).cloned().ok_or(()))
                .ok()
                .and_then(|resolved| len_of(&resolved))
                .unwrap_or(0),
        },
    }
}

/// Walk an operation's **root** selection set, following fragments, and hand
/// every field to `visit`. `seen` breaks a self-referential fragment's cycle:
/// the document is not validated yet.
fn visit_root_fields<'d>(
    document: &'d ExecutableDocument,
    selection_set: &'d Positioned<SelectionSet>,
    seen: &mut HashSet<&'d str>,
    visit: &mut dyn FnMut(&Positioned<async_graphql::parser::types::Field>) -> ServerResult<()>,
) -> ServerResult<()> {
    for selection in &selection_set.node.items {
        match &selection.node {
            Selection::Field(field) => visit(field)?,
            Selection::InlineFragment(fragment) => {
                visit_root_fields(document, &fragment.node.selection_set, seen, visit)?;
            }
            Selection::FragmentSpread(spread) => {
                if !seen.insert(spread.node.fragment_name.node.as_str()) {
                    continue;
                }
                if let Some(fragment) = document.fragments.get(&spread.node.fragment_name.node) {
                    visit_root_fields(document, &fragment.node.selection_set, seen, visit)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use async_graphql::parser::parse_query;
    use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
    use nest_rs_testing::LogCapture;

    use super::*;
    use crate::testing::thread_cpu_time;

    struct Query;

    #[Object]
    impl Query {
        async fn ping(&self) -> bool {
            true
        }
    }

    /// A root may spread tens of thousands of fragments, each a look at what the
    /// read already holds and never a scan of it: the read costs a fraction of the
    /// parse it reads, on this thread's CPU clock, where a scan cost half of it here
    /// and more than all of it at twice the size.
    #[test]
    fn counting_a_roots_representations_costs_a_fraction_of_parsing_it() {
        let spreads: String = (0..8_000).map(|at| format!(" ...B{at}")).collect();
        let bodies: String = (0..8_000)
            .map(|at| format!(" fragment B{at} on Query {{ ping }}"))
            .collect();
        let text = format!("query {{{spreads} }}{bodies}");

        let started = thread_cpu_time();
        let document = parse_query(&text).expect("the document parses");
        let parsing = thread_cpu_time() - started;
        let started = thread_cpu_time();
        refuse_oversized_entity_calls(&document, &Variables::default(), None, 100)
            .expect("it calls `_entities` nowhere");
        let counting = thread_cpu_time() - started;

        assert!(
            counting * 4 < parsing,
            "parsed in {parsing:?} of this thread's CPU, counted in {counting:?}"
        );
    }

    /// A schema `GraphqlModule` did not build carries no container: the
    /// federation fields are refused rather than served past their guards.
    #[tokio::test]
    async fn a_federation_field_without_a_container_is_refused_opaquely() {
        let schema = Schema::build(Query, EmptyMutation, EmptySubscription)
            .enable_federation()
            .extension(FederationExtensionFactory {
                max_representations: None,
            })
            .finish();
        let logs = LogCapture::install();

        let response = schema.execute(Request::new("{ _service { sdl } }")).await;

        let body = serde_json::to_value(&response).expect("a response serializes");
        assert!(!body.to_string().contains("type Query"), "{body}");
        assert_eq!(
            response.errors.first().map(|error| error.message.as_str()),
            Some(nest_rs_core::OPAQUE_CLIENT_MESSAGE),
            "{body}",
        );
        let line = logs.expect_one(crate::TARGET, "graphql operation failed");
        assert!(
            line.field("error")
                .is_some_and(|error| error.contains(SERVICE_FIELD)),
            "{:?}",
            line.fields,
        );
    }
}
