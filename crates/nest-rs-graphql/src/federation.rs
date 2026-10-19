//! What runs the two federation root fields — each its own site on the
//! pipeline — and the ceiling on how many references one call may carry.
//!
//! `_service` and `_entities` are resolved by async-graphql's own `QueryRoot`,
//! *above* [`DiscoveredQuery`](crate::resolver::DiscoveredQuery), so no
//! resolver's wrapper sees them. A schema [`Extension`] is the one seam in
//! front of a root field: the field's unit runs on `resolve`, the ceiling on
//! `parse_query`.
//!
//! A federation field belongs to no resolver, so its guard stage is the
//! app-wide pool alone, which an `#[entity]`'s own site therefore stops
//! composing.

use std::any::TypeId;
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
use nest_rs_core::__private::{
    GuardStage, HandlerDeclaration, MetaLevels, Site, SiteDeclaration, SiteLayers, declare_handler,
    declare_site,
};
use nest_rs_core::anyhow;
use nest_rs_core::{Container, Handler, Posture};
use nest_rs_guards::__private::resolve_global_chain;
use nest_rs_guards::GuardSpecs;

use crate::config::GraphqlConfig;
use crate::error::FieldFailure;
use crate::operation::{FieldGuardStage, FieldUnit, Floor, ROOT_FIELD, in_field_unit};
use crate::view::FieldRole;

/// The `Query`-root field a router calls to read a subgraph's SDL.
pub(crate) const SERVICE_FIELD: &str = "_service";
/// The `Query`-root field a router calls with entity references.
pub(crate) const ENTITIES_FIELD: &str = "_entities";
/// Its one argument, whose length is the fan-out.
const REPRESENTATIONS_ARG: &str = "representations";

/// Whether the federation root fields run the app-wide guard pool: whenever
/// the app lists one, an empty list included.
pub(crate) fn gate_runs(container: &Container) -> bool {
    container.get::<GuardSpecs>().is_some()
}

/// The handler `_service` runs as.
static SERVICE: Handler = federation_field(SERVICE_FIELD);
/// The handler `_entities` runs as.
static ENTITIES: Handler = federation_field(ENTITIES_FIELD);

/// The site `_service` runs.
static SERVICE_SITE: Site = declare_site(&SERVICE, FEDERATION_FIELD);
/// The site `_entities` runs.
static ENTITIES_SITE: Site = declare_site(&ENTITIES, FEDERATION_FIELD);

/// A federation root field's declaration: a root field no resolver declares,
/// gated by the app-wide pool alone, inside the app's filters and
/// interceptors.
const FEDERATION_FIELD: SiteDeclaration = SiteDeclaration {
    layers: SiteLayers::default,
    global: ROOT_FIELD,
    guard_stage: Some(federation_guard_stage),
    class_gate: None,
};

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

/// The app-wide pool, the whole chain of a field that belongs to no resolver;
/// none when the app lists no pool, so an `#[entity]` folds it itself.
fn federation_guard_stage(
    container: &Container,
    _: &Handler,
) -> anyhow::Result<Option<Arc<dyn GuardStage>>> {
    if !gate_runs(container) {
        return Ok(None);
    }
    let chain = resolve_global_chain(container, "POST /graphql (federation)");
    Ok(FieldGuardStage::of(chain, Floor::Federation))
}

/// Compose both federation sites against `container`, so a layer they fold
/// that does not resolve refuses the boot.
pub(crate) fn compose_sites(container: &Container) -> anyhow::Result<()> {
    SERVICE_SITE.chains(container)?;
    ENTITIES_SITE.chains(container)?;
    Ok(())
}

/// Installs [`FederationExtension`] on a subgraph's schema.
pub(crate) struct FederationExtensionFactory {
    max_representations: Option<usize>,
}

impl FederationExtensionFactory {
    /// `None` unless the schema is a subgraph: only a subgraph serves the
    /// federation fields, and an idle extension costs every field a boxed
    /// indirection.
    pub(crate) fn for_config(config: &GraphqlConfig) -> Option<Self> {
        // A pinned `Some(0)` never passes through `ConfigService::count`, which
        // maps the environment's `0` to unlimited.
        config.federation.then(|| Self {
            max_representations: config.max_representations.filter(|max| *max > 0),
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
        let Some(container) = ctx.data_opt::<Container>() else {
            return next.run(ctx, info).await;
        };
        let position = info.field.name.pos;
        let field = info.name;
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
        in_field_unit("federation", field, async move {
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
        })
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
    let mut seen = Vec::new();
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
    let key = nest_rs_config::var_name("graphql", "MAX_REPRESENTATIONS");
    Err(ServerError::new(
        format!(
            "this operation carries {total} `_entities` representations, over this \
             schema's ceiling of {max}. Each one is resolved on its own — body, \
             posture gate and mask — so the total list length is the fan-out, \
             however many `_entities` fields it is spread across. Split the call, \
             or raise `GraphqlConfig::max_representations` (`{key}`; `0` ⇒ \
             unlimited).",
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
fn visit_root_fields(
    document: &ExecutableDocument,
    selection_set: &Positioned<SelectionSet>,
    seen: &mut Vec<String>,
    visit: &mut dyn FnMut(&Positioned<async_graphql::parser::types::Field>) -> ServerResult<()>,
) -> ServerResult<()> {
    for selection in &selection_set.node.items {
        match &selection.node {
            Selection::Field(field) => visit(field)?,
            Selection::InlineFragment(fragment) => {
                visit_root_fields(document, &fragment.node.selection_set, seen, visit)?;
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.as_str();
                if seen.iter().any(|s| s == name) {
                    continue;
                }
                seen.push(name.to_owned());
                if let Some(fragment) = document.fragments.get(&spread.node.fragment_name.node) {
                    visit_root_fields(document, &fragment.node.selection_set, seen, visit)?;
                }
            }
        }
    }
    Ok(())
}
