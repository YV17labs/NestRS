//! What one GraphQL operation *is*, for the layers that run around it.
//!
//! A `#[query]` body is handed async-graphql's `Context`, which carries the
//! selected field and every scope of request data. The two **federation** root
//! fields are handed nothing of the sort: `_service` and `_entities` are
//! resolved by async-graphql's own `QueryRoot`, *above* the merged root this
//! crate builds, and the only place a framework can stand in front of them is an
//! [`Extension`](async_graphql::extensions::Extension) — which async-graphql
//! hands an [`ExtensionContext`], never a `Context`, with no public constructor
//! to bridge the two.
//!
//! [`GraphqlOperationContext`] is that bridge, and it is why `check_graphql`
//! takes it rather than a bare `Context`: **one declaration, both sites**. What
//! a guard reads on either is the same — the request-scoped data a
//! [`GraphqlContextSeed`](crate::GraphqlContextSeed) forwarded, the container,
//! the field's name. What only the field site can offer — the selection set, the
//! arguments, `look_ahead` — is behind [`context`](GraphqlOperationContext::context),
//! which answers `None` on a federation field rather than pretending.

use std::any::Any;

use async_graphql::extensions::ExtensionContext;
use async_graphql::{Context, Result};
use tracing::Instrument;

/// One GraphQL operation, as a [`Guard`](https://docs.rs/nest-rs-guards) sees
/// it — the GraphQL analog of the [`McpOperationContext`] a `check_mcp` takes
/// and the `(client, event, data)` a `check_ws_message` takes.
///
/// [`McpOperationContext`]: https://docs.rs/nest-rs-mcp
pub struct GraphqlOperationContext<'a> {
    site: Site<'a>,
}

/// The two places async-graphql lets a check run, and everything that differs
/// between them.
enum Site<'a> {
    /// A resolver field — `#[query]`, `#[mutation]`, `#[entity]`,
    /// `#[field_resolver]`. Emitted inline by `#[operations]`.
    Field(&'a Context<'a>),
    /// A federation root field, from the schema extension. It belongs to no
    /// resolver — the router calls it on the schema itself — so the name is a
    /// literal rather than something read off a selected field.
    Federation {
        ctx: &'a ExtensionContext<'a>,
        field: &'static str,
    },
}

impl<'a> GraphqlOperationContext<'a> {
    /// The operation a resolver field is about to run.
    pub fn field(ctx: &'a Context<'a>) -> Self {
        Self {
            site: Site::Field(ctx),
        }
    }

    /// The operation a federation root field is about to run.
    ///
    /// Framework-emitted: the two names are async-graphql's, not an app's.
    #[doc(hidden)]
    pub fn federation(ctx: &'a ExtensionContext<'a>, field: &'static str) -> Self {
        Self {
            site: Site::Federation { ctx, field },
        }
    }

    /// The field being resolved, as the client wrote it in the document.
    pub fn name(&self) -> &str {
        match &self.site {
            Site::Field(ctx) => ctx.item.node.name.node.as_str(),
            Site::Federation { field, .. } => field,
        }
    }

    /// The full async-graphql context, when this operation has one.
    ///
    /// `None` on a federation root field — see the module docs. A guard reaching
    /// for arguments or a selection set there is asking a question the site
    /// cannot answer, and gets to decide what that means rather than being
    /// handed a fabricated context.
    pub fn context(&self) -> Option<&'a Context<'a>> {
        match &self.site {
            Site::Field(ctx) => Some(ctx),
            Site::Federation { .. } => None,
        }
    }

    /// Request- or schema-scoped data, or `None`.
    pub fn data_opt<D: Any + Send + Sync>(&self) -> Option<&'a D> {
        match &self.site {
            Site::Field(ctx) => ctx.data_opt::<D>(),
            Site::Federation { ctx, .. } => ctx.data_opt::<D>(),
        }
    }

    /// Request- or schema-scoped data, or an error naming the missing type.
    pub fn data<D: Any + Send + Sync>(&self) -> Result<&'a D> {
        match &self.site {
            Site::Field(ctx) => ctx.data::<D>(),
            Site::Federation { ctx, .. } => ctx.data::<D>(),
        }
    }

    /// Request- or schema-scoped data. Panics when absent — for data the
    /// framework guarantees, like the [`Container`](nest_rs_core::Container).
    pub fn data_unchecked<D: Any + Send + Sync>(&self) -> &'a D {
        match &self.site {
            Site::Field(ctx) => ctx.data_unchecked::<D>(),
            Site::Federation { ctx, .. } => ctx.data_unchecked::<D>(),
        }
    }
}

/// Run one dispatched GraphQL field as its own unit of work.
///
/// Framework-emitted: `#[operations]` wraps every `#[query]`, `#[mutation]`,
/// `#[entity]` and `#[field_resolver]` body in this, so a resolver author writes
/// nothing. It is one seam rather than four because the four differ only in the
/// `role` they pass — bolting the line onto whichever role asked for it is the
/// defect the family rule names.
///
/// # Why this exists at all
///
/// A GraphQL document arrives as one `POST /graphql`, so until this ran, every
/// query and mutation in a deployment was the same line — `POST /graphql 200` —
/// and which field was slow, which one failed, and which one the caller was
/// refused at were all unanswerable from the console. The HTTP request is the
/// *transport's* unit; the field is this edge's, and this crate is the only
/// place that boundary is visible. MCP is the same shape and already did it:
/// an HTTP self-mount that dispatches in band files its own operation line
/// **in addition to** the request's.
///
/// A subscription is deliberately not here — see [`crate::unit`].
///
/// # What it carries, and what it does not
///
/// `role` and `operation`, flat, plus the family's `outcome` and `duration_ms`.
/// `operation` is the field name **as the client wrote it in the document**,
/// read through [`GraphqlOperationContext::name`] rather than from the Rust
/// method's ident: async-graphql renames `list_users` to `listUsers` on the
/// wire, and a line naming the ident cannot be joined against a capture of the
/// request that produced it.
///
/// No `outcome = panic`: a resolver that unwinds never reaches this line, and
/// async-graphql owns the catch. Reporting one would be a claim this seam
/// cannot make.
#[doc(hidden)]
pub async fn run_operation<T, F>(
    role: &'static str,
    operation: &str,
    succeeded: impl FnOnce(&T) -> bool,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    // A child of the request that carried the document: same trace, a fresh
    // span, the request's as its parent — the shape `mcp.operation` and
    // `ws.message` already have. Minted outright only where nothing carried it,
    // which off a request task is the honest answer rather than a missing id.
    let correlation = match nest_rs_core::current_correlation() {
        Some(request) => request.child(),
        None => nest_rs_core::Correlation::mint(),
    };
    let span = nest_rs_core::operation_span!(
        target: crate::TARGET,
        kind: nest_rs_core::operation_log::kind::SERVER,
        crate::unit::OPERATION,
        &correlation,
        // Dotted and conventions-shaped on the span, flat on the line — the
        // split `nest_rs_core::operation_log` states.
        //
        // **Two deviations from OpenTelemetry's GraphQL server conventions, and
        // they are stated rather than silent.** That semconv defines
        // `graphql.operation.name`, `graphql.operation.type` (an enum of
        // `query` / `mutation` / `subscription`) and `graphql.document`.
        //
        // `graphql.operation.role` is not `graphql.operation.type` renamed: it
        // carries a **superset** — `entity` and `field_resolver` besides the
        // three — because this framework dispatches at the *field*, and a
        // federation `#[entity]` is a unit of work the specification has no word
        // for. Recording that superset under the conventions' key would answer
        // an enum with a value outside it, which is worse for a backend than a
        // key it does not know.
        //
        // `graphql.field.name` is the field, not the operation: the conventions'
        // `operation.name` is the *document's* name, which is the caller's
        // label for a batch of fields and is often absent. What names this unit
        // is the field it resolves, and there is no conventions key for it.
        //
        // `graphql.document` is deliberately absent: it is the caller's query
        // text, which carries their literals.
        graphql.operation.role = role,
        graphql.field.name = operation,
    );
    let started = std::time::Instant::now();
    // The request's own scope, under this unit's correlation: a field resolves
    // against the request that asked for it — its providers, its executor, its
    // ability — while its events and its line name the field rather than the
    // whole document.
    nest_rs_core::with_request_scope(
        nest_rs_core::current_request_scope(),
        correlation,
        async move {
            let out = fut.await;
            // Filed inside the scope, so it carries this unit's ids without
            // being handed them — the shape `nest_rs_schedule`'s tick uses.
            tracing::info!(
                name: crate::unit::OPERATION,
                target: nest_rs_core::operation_log::TARGET,
                message = crate::unit::OPERATION,
                role = role,
                operation = operation,
                outcome = if succeeded(&out) {
                    nest_rs_core::operation_log::OK
                } else {
                    nest_rs_core::operation_log::ERROR
                },
                duration_ms = nest_rs_core::operation_log::duration_ms(started),
            );
            out
        },
    )
    .instrument(span)
    .await
}
