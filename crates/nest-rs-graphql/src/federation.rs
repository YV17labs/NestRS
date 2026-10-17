//! What stands in front of the two federation root fields — the guard chain, and
//! the ceiling on how many references one call may carry.
//!
//! `_service` and `_entities` are resolved by async-graphql's own `QueryRoot`,
//! *above* [`DiscoveredQuery`](crate::resolver::DiscoveredQuery), so no
//! resolver's chain sees them. A schema [`Extension`] is the one seam in front of
//! a root field: the chain runs on `resolve`, the ceiling on `parse_query`.
//!
//! A federation field belongs to no resolver, so its chain is the app-wide pool
//! alone, which an `#[entity]`'s own site therefore stops composing.

use std::sync::{Arc, Mutex};

use async_graphql::async_trait::async_trait;
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextParseQuery, NextPrepareRequest, NextResolve,
    ResolveInfo,
};
use async_graphql::parser::types::{ExecutableDocument, OperationType, Selection, SelectionSet};
use async_graphql::{Name, Positioned, Request, ServerError, ServerResult, Value, Variables};
use std::any::TypeId;

use nest_rs_core::__private::{
    HandlerDeclaration, MetaLevels, ResolvedLayer, declare_handler, install_pending,
    new_unit_context,
};
use nest_rs_core::{Container, Handler, Posture};
use nest_rs_guards::__private::{failure_chain, resolve_global_chain, run_stage};
use nest_rs_guards::{Guard, GuardSpecs};

use crate::config::GraphqlConfig;
use crate::error::denial_to_graphql_error;
use crate::view::{FieldRole, GraphqlView};

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

/// The site `_service` runs as.
static SERVICE: Handler = federation_field(SERVICE_FIELD);
/// The site `_entities` runs as.
static ENTITIES: Handler = federation_field(ENTITIES_FIELD);

/// A federation root field's declaration: a field no resolver declares, gated
/// by the app-wide pool alone.
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

/// Installs [`FederationExtension`] on the served schema.
pub(crate) struct FederationExtensionFactory {
    pool: Option<Arc<[ResolvedLayer<dyn Guard>]>>,
    max_representations: Option<usize>,
}

impl FederationExtensionFactory {
    /// `None` when the app declared no global guards **and** left the ceiling
    /// unlimited: an idle extension still costs every field a boxed indirection.
    pub(crate) fn from_container(container: &Container, config: &GraphqlConfig) -> Option<Self> {
        let pool = gate_runs(container)
            .then(|| resolve_global_chain(container, "POST /graphql (federation)").into());
        // A pinned `Some(0)` never passes through `ConfigService::count`, which
        // maps the environment's `0` to unlimited.
        let max_representations = config.max_representations.filter(|max| *max > 0);
        (pool.is_some() || max_representations.is_some()).then_some(Self {
            pool,
            max_representations,
        })
    }
}

impl ExtensionFactory for FederationExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(FederationExtension {
            pool: self.pool.clone(),
            max_representations: self.max_representations,
            operation_name: Mutex::new(None),
        })
    }
}

struct FederationExtension {
    /// The app-wide pool, the whole chain of a field that belongs to no
    /// resolver.
    pool: Option<Arc<[ResolvedLayer<dyn Guard>]>>,
    max_representations: Option<usize>,
    /// The operation this request selected, captured in `prepare_request` — the
    /// one hook that carries it — so the ceiling can be charged to the operation
    /// that will actually run. An extension instance is built per request
    /// (`ExtensionFactory::create`), so this holds one request's answer.
    operation_name: Mutex<Option<String>>,
}

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
            ("Query", SERVICE_FIELD) => Some(&SERVICE),
            ("Query", ENTITIES_FIELD) => Some(&ENTITIES),
            _ => None,
        };
        let (Some(site), Some(pool)) = (site, self.pool.as_deref()) else {
            return next.run(ctx, info).await;
        };
        let mut view =
            GraphqlView::new(site.address(), OperationType::Query, FieldRole::Federation);
        let mut cx = new_unit_context(site, &mut view, None, None);
        let attached = run_stage(pool, None, &mut cx).await.map_err(|refused| {
            let denial = refused.into_denial(|guard, denial| {
                // Same structural floor as a resolver site's chain.
                tracing::warn!(
                    target: nest_rs_core::target::LAYERS,
                    guard,
                    route = "POST /graphql (federation)",
                    field = site.address(),
                    status = denial.http_status(),
                    error = failure_chain(denial),
                    "guard denied the federation field",
                );
            });
            denial_to_graphql_error(denial).into_server_error(info.field.name.pos)
        })?;
        install_pending(attached, next.run(ctx, info)).await
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
