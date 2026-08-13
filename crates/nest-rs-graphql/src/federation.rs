//! What stands in front of the two federation root fields — the guard chain, and
//! the ceiling on how many references one call may carry.
//!
//! `_service` and `_entities` are resolved by async-graphql's own `QueryRoot`,
//! *above* [`DiscoveredQuery`](crate::resolver): the guard chain `#[operations]`
//! emits inside a resolver body never sees them. For `_entities` that showed as
//! a chain running once **per representation**, inside whichever member the
//! router's reference happened to match; for `_service` it showed as nothing at
//! all — a `check_graphql` deny-all pool returned the endpoint's entire SDL,
//! `NESTRS_GRAPHQL__DISABLE_INTROSPECTION` notwithstanding, since that switch
//! covers `__schema` and not this.
//!
//! A schema [`Extension`] is the one seam async-graphql leaves in front of a
//! root field, so both live here — the chain on `resolve`, once per field and
//! before `QueryRoot` resolves anything, and the ceiling on `parse_query`,
//! before a document with a hundred thousand references is executed at all.
//!
//! **Which chain, and why the app-wide one is the whole answer here.** A
//! federation field belongs to no resolver: the router calls it on the schema,
//! not on a provider, so there is no `#[use_guards]` scope to compose and no
//! posture to read. What is left is exactly the pool `use_guards_global`
//! declared — and that is what an `#[entity]`'s own site therefore stops
//! composing, since every `#[entity]` is reached through `_entities` and would
//! otherwise run the same pooled guard a second time, once per representation.

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

/// Gate the two federation root fields carry — the seam `nest-rs-guards` fills
/// with the app-wide guard pool.
///
/// Defined here and implemented there for the same reason
/// [`GraphqlOperationGuard`](crate::GraphqlOperationGuard) is: guards depends on
/// this crate, not the reverse.
pub trait GraphqlFederationGuard: Send + Sync + 'static {
    /// Run the app-wide chain against a federation root field.
    fn check<'a>(
        &'a self,
        operation: &'a GraphqlOperationContext<'a>,
    ) -> BoxFuture<'a, Result<(), GraphqlError>>;
}

/// Factory slot for the [`GraphqlFederationGuard`]. `nest-rs-guards`'
/// `use_guards_global` seeds one (a fn pointer — the container does not exist
/// yet at builder time); the schema build invokes it at mount.
///
/// **Internal ABI** — a seeded fn-pointer wired by the framework crates
/// (lockstep with `nest-rs-guards`); not a user-constructed type.
#[doc(hidden)]
pub struct FederationGate(pub fn(&Container) -> Arc<dyn GraphqlFederationGuard>);

/// Installs [`FederationExtension`] on the served schema.
pub(crate) struct FederationExtensionFactory {
    guard: Option<Arc<dyn GraphqlFederationGuard>>,
    max_representations: Option<usize>,
}

impl FederationExtensionFactory {
    /// `None` when the app declared no global guards **and** left the ceiling
    /// unlimited — there is nothing for the extension to do, and an extension
    /// that does nothing still costs every field the boxed indirection
    /// `Fields::add_set` takes when the chain is non-empty.
    pub(crate) fn from_container(container: &Container, config: &GraphqlConfig) -> Option<Self> {
        let guard = container
            .get::<FederationGate>()
            .map(|gate| (gate.0)(container));
        // `0` is this field's documented "unlimited" sentinel, and it has to mean
        // that on **both** paths. `ConfigService::count` maps it from the
        // environment; a `Some(0)` pinned in a `GraphqlConfig` never passes
        // through `count` at all, and reached the comparison as a ceiling of zero
        // — refusing every `_entities` call, with an error naming `0` as the fix.
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
        // A poisoned lock must not deny service: the value is advisory — losing
        // it only widens the ceiling's reading back to the first operation.
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
            // The guard is held across the call and not cloned out of: the
            // refusal is synchronous, so there is no await to hold a lock over.
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
        // `parent_type` narrows it to the two root fields the federation spec
        // adds: nothing else in a schema may carry a leading underscore, but a
        // *nested* field is a different operation with its own chain already.
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

/// Refuse an operation whose `_entities` calls carry more references, **in
/// total**, than `max`.
///
/// Read off the parsed document rather than the resolved argument, because this
/// is the last place the whole list is one value: by the time `find_entity`
/// runs, async-graphql has already launched one future per element.
///
/// **The total, not each field.** `_entities` may be aliased without limit, and
/// counting per field left the fan-out exactly where it was — a 29 KB document
/// of five hundred aliases each at the ceiling resolved fifty thousand
/// references and raised no error. The number that has to be bounded is what one
/// operation asks the schema to resolve.
///
/// **The operation that will run, not every operation in the document.**
/// `operation_name` is captured in `prepare_request`, which is the only hook
/// carrying it; without it a document holding a cheap selected operation beside
/// a fat unselected one was refused for the one it never executed.
///
/// One deliberate over-count remains: a `_entities` the executor would strip
/// under `@skip(if: true)` is still counted. Reproducing async-graphql's skip
/// rule here would be a second implementation of it — the drift this repo names
/// as a defect class — and the error is the safe direction, refusing a request
/// that would have cost less than it said.
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
        // Unnamed: async-graphql runs the single operation, whatever it is
        // called. With several and no name the request is an error of its own —
        // counting them all is the safe reading of a document that will not run.
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

/// How many references one `_entities` field carries.
///
/// The cheap path first, because it is the one a router takes: a bare
/// `$representations` clones only the variable's *name* before the lookup. An
/// inline list, or a list whose elements are variables, falls through to the
/// full resolve — the only spelling that counts `[$a, $b]` correctly — and pays
/// a copy bounded by the transport's body cap.
///
/// `into_const_with` rather than a match on the AST value's own variants: that
/// enum is `async_graphql_value`'s, and async-graphql re-exports only its *const*
/// half (`Value` here), so naming the unresolved one would mean a second direct
/// dependency pinned in lockstep with the first.
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
    // Neither a list nor a variable bound to one is async-graphql's own coercion
    // error to report; a ceiling breach here would name the wrong defect.
    let len_of = |value: &Value| match value {
        Value::List(items) => Some(items.len()),
        _ => None,
    };
    // Cloned once, then consumed by whichever branch runs. A bare
    // `$representations` copies only the variable's `Name` (an `Arc<str>`); an
    // inline list copies its elements, bounded by the transport's body cap.
    let owned = value.node.clone();
    match owned.clone().into_const_with::<Name>(Err) {
        Ok(resolved) => len_of(&resolved).unwrap_or(0),
        Err(name) => match variables.get(&name).and_then(len_of) {
            Some(len) => len,
            // A variable somewhere *inside* the value rather than the value
            // itself — `[$a, $b]`. Resolving is the only spelling that counts it.
            None => owned
                .into_const_with::<()>(|name| variables.get(&name).cloned().ok_or(()))
                .ok()
                .and_then(|resolved| len_of(&resolved))
                .unwrap_or(0),
        },
    }
}

/// Walk an operation's **root** selection set, following fragments, and hand
/// every field to `visit`.
///
/// Root only: `_entities` is a `Query`-root field, and a same-named field on
/// some other type is an app's own, with its own chain. `seen` breaks the cycle
/// a self-referential fragment would otherwise make — the document is
/// unvalidated at this point, so the parser's output is not yet known to be
/// acyclic.
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
