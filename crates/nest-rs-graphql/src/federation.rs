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
use async_graphql::{
    Error as GraphqlError, Name, Positioned, Request, ServerError, ServerResult, Value, Variables,
};
use nest_rs_core::Container;

use crate::config::GraphqlConfig;
use crate::context::BoxFuture;
use crate::operation::GraphqlOperationContext;

/// The `Query`-root field a router calls to read a subgraph's SDL.
pub(crate) const SERVICE_FIELD: &str = "_service";
/// The `Query`-root field a router calls with entity references.
pub(crate) const ENTITIES_FIELD: &str = "_entities";
/// Its one argument, whose length is the fan-out.
const REPRESENTATIONS_ARG: &str = "representations";

/// Gate the two federation root fields carry — the seam `nest-rs-guards` (which
/// depends on this crate) fills with the app-wide guard pool.
pub trait GraphqlFederationGuard: Send + Sync + 'static {
    /// Run the app-wide chain against a federation root field.
    fn check<'a>(
        &'a self,
        operation: &'a GraphqlOperationContext<'a>,
    ) -> BoxFuture<'a, Result<(), GraphqlError>>;
}

/// Factory slot for the [`GraphqlFederationGuard`], seeded by `nest-rs-guards`'
/// `use_guards_global` and invoked at mount.
///
/// **Internal ABI** — wired by the framework crates in lockstep.
#[doc(hidden)]
pub struct FederationGate(pub fn(&Container) -> Arc<dyn GraphqlFederationGuard>);

/// Installs [`FederationExtension`] on the served schema.
pub(crate) struct FederationExtensionFactory {
    guard: Option<Arc<dyn GraphqlFederationGuard>>,
    max_representations: Option<usize>,
}

impl FederationExtensionFactory {
    /// `None` when the app declared no global guards **and** left the ceiling
    /// unlimited: an idle extension still costs every field a boxed indirection.
    pub(crate) fn from_container(container: &Container, config: &GraphqlConfig) -> Option<Self> {
        let guard = container
            .get::<FederationGate>()
            .map(|gate| (gate.0)(container));
        // A pinned `Some(0)` never passes through `ConfigService::count`, which
        // maps the environment's `0` to unlimited.
        let max_representations = config.max_representations.filter(|max| *max > 0);
        (guard.is_some() || max_representations.is_some()).then_some(Self {
            guard,
            max_representations,
        })
    }
}

impl ExtensionFactory for FederationExtensionFactory {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(FederationExtension {
            guard: self.guard.clone(),
            max_representations: self.max_representations,
            operation_name: Mutex::new(None),
        })
    }
}

struct FederationExtension {
    guard: Option<Arc<dyn GraphqlFederationGuard>>,
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
        let field = match (info.parent_type, info.name) {
            ("Query", SERVICE_FIELD) => Some(SERVICE_FIELD),
            ("Query", ENTITIES_FIELD) => Some(ENTITIES_FIELD),
            _ => None,
        };
        if let (Some(field), Some(guard)) = (field, self.guard.as_ref()) {
            let operation = GraphqlOperationContext::federation(ctx, field);
            if let Err(err) = guard.check(&operation).await {
                return Err(err.into_server_error(info.field.name.pos));
            }
        }
        next.run(ctx, info).await
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
