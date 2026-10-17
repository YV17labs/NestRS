//! What runs around one GraphQL field a resolver declares: its guard chain,
//! composed once per site, and — for a root field — its own unit of work.

use async_graphql::parser::types::OperationType;
use async_graphql::{Context, Error as GraphqlError};
use nest_rs_core::__private::{Pending, new_unit_context};
use nest_rs_core::{Container, Handler};
use nest_rs_guards::__private::{
    GlobalBucket, SiteChainCell, SiteChainSources, failure_chain, run_stage,
};
use tracing::Instrument;

use crate::error::denial_to_graphql_error;
use crate::view::{FieldRole, GraphqlView};

/// Which GraphQL site is running the chain — what decides whether it folds the
/// app-wide pool, and the role its fields play.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GraphqlSite {
    /// A `#[query]` / `#[mutation]` / `#[subscription]`: the app-wide pool runs
    /// here, each pooled guard's `check`, which the `/graphql` request's
    /// `check_http` never stands in for.
    Operation,
    /// A `#[field_resolver]`: its resolver's and method's guards run, the pool
    /// does not — the root field of the same request already ran it, and
    /// folding it again would run it once per parent.
    Field,
    /// An `#[entity]`, reached only through `_entities`, whose federation gate
    /// runs the pool once per field; folding it here would run it once per
    /// representation.
    Entity,
}

impl GraphqlSite {
    /// Whether this site composes the pool, read off the container so the memo
    /// cell's key still covers the whole composition.
    ///
    /// An entity skips it only where the federation gate runs it: a global
    /// pool can be absent, and skipping then would leave nothing to skip.
    fn bucket(self) -> fn(&Container) -> GlobalBucket {
        match self {
            Self::Operation => |_| GlobalBucket::Fold,
            Self::Field => |_| GlobalBucket::Skip,
            Self::Entity => |container| {
                if crate::federation::gate_runs(container) {
                    GlobalBucket::Skip
                } else {
                    GlobalBucket::Fold
                }
            },
        }
    }

    /// The role this site's fields play.
    fn role(self) -> FieldRole {
        match self {
            Self::Operation => FieldRole::RootField,
            Self::Field => FieldRole::Nested,
            Self::Entity => FieldRole::Entity,
        }
    }
}

/// Run a resolver site's guard chain — called by `#[operations]` at the start
/// of every resolver method, `handler` the site's declaration. Dedups the
/// resolver's and the method's guards against the global chain, then runs each
/// guard's `check` on the field: what they attached comes back, for the method
/// to run under.
///
/// `sources` is consulted only when the chain is composed. A `&dyn Fn` rather
/// than an `impl Fn`, so this body codegens once rather than per resolver.
/// GraphQL pipes run at the transport's request entry, not here.
pub async fn run_layered_graphql_chain(
    ctx: &Context<'_>,
    container: &Container,
    cell: &SiteChainCell,
    route_label: &str,
    sources: &(dyn Fn() -> SiteChainSources + Sync),
    site: GraphqlSite,
    handler: &'static Handler,
) -> Result<Pending, GraphqlError> {
    let chain = cell
        .chain(container, route_label, sources, site.bucket())
        .map_err(denial_to_graphql_error)?;
    // The executing operation's kind: async-graphql pins it on the query
    // environment it hands each field (`=7.2.1`, the workspace's pin).
    let operation: OperationType = ctx.query_env.operation.node.ty;
    let mut view = GraphqlView::new(handler.address(), operation, site.role());
    let mut cx = new_unit_context(handler, &mut view, None, None);
    run_stage(&chain, &mut cx).await.map_err(|(guard, denial)| {
        // Structural floor mirroring the HTTP edge's: every denial visible at
        // warn+ regardless of what the individual guard logged.
        tracing::warn!(
            target: nest_rs_core::target::LAYERS,
            guard,
            route = route_label,
            status = denial.http_status(),
            error = failure_chain(&denial),
            "guard denied the operation",
        );
        denial_to_graphql_error(denial)
    })
}

/// The field `ctx` resolves, as the client wrote it in the document (`listUsers`,
/// never an alias).
pub fn field_name<'a>(ctx: &'a Context<'_>) -> &'a str {
    ctx.item.node.name.node.as_str()
}

/// Run one dispatched root field as its own unit of work; `#[operations]` wraps
/// every `#[query]`, `#[mutation]` and `#[entity]` body in this. A
/// `#[field_resolver]` call is no unit: it runs once per row its parent returns,
/// so a line per call would file one per row.
///
/// `operation` is the field name **as the client wrote it** (`listUsers`, not
/// `list_users`), so the line joins against a capture of the request.
///
/// The line files `ok` or `error` on return, `cancelled` when dropped first, and
/// `panic` when it unwinds — caught only to be named, then resumed. A sibling
/// torn down by that unwind files `cancelled`: one panic is one `panic` line.
pub async fn run_field<T, F>(
    role: &'static str,
    operation: &str,
    succeeded: impl FnOnce(&T) -> bool,
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let correlation = match nest_rs_core::__private::current_correlation() {
        Some(request) => request.child(),
        None => nest_rs_core::Correlation::minted(None),
    };
    let span = nest_rs_core::operation_span!(
        crate::unit::FIELD,
        &correlation,
        // Not OTel's `graphql.operation.type`: `role` adds `entity` to its enum.
        // `graphql.document` is omitted: it carries the caller's literals.
        graphql.operation.role = role,
        graphql.field.name = operation,
    );
    let line = FieldLine {
        role,
        operation,
        correlation: correlation.clone(),
        span: span.clone(),
        started: std::time::Instant::now(),
        filed: false,
    };
    nest_rs_core::with_request_scope(
        nest_rs_core::current_request_scope(),
        correlation,
        async move {
            match nest_rs_core::panic::contain(fut).await {
                Ok(out) => {
                    line.file(if succeeded(&out) {
                        nest_rs_core::operation_log::OK
                    } else {
                        nest_rs_core::operation_log::ERROR
                    });
                    out
                }
                Err(unwound) => {
                    line.file(nest_rs_core::operation_log::PANIC);
                    std::panic::resume_unwind(unwound)
                }
            }
        },
    )
    .instrument(span)
    .await
}

/// One root field's `graphql.field` line, filed exactly once, by `Drop` as
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED) when dropped first.
///
/// The correlation is held: a `Drop` runs while the future is torn down, not
/// reliably inside the scope that future installed.
struct FieldLine<'a> {
    role: &'static str,
    operation: &'a str,
    correlation: nest_rs_core::Correlation,
    span: tracing::Span,
    started: std::time::Instant,
    filed: bool,
}

impl FieldLine<'_> {
    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
    }

    fn emit(&mut self, outcome: &'static str) {
        self.filed = true;
        nest_rs_core::RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            nest_rs_core::operation_line!(
                crate::unit::FIELD,
                span: &self.span,
                outcome: outcome,
                started: self.started,
                role = self.role,
                operation = self.operation,
            );
        });
    }
}

impl Drop for FieldLine<'_> {
    fn drop(&mut self) {
        if !self.filed {
            self.emit(nest_rs_core::operation_log::CANCELLED);
        }
    }
}

/// A `#[subscription]`'s answer, accepted only when it is a value — a stream.
/// async-graphql's subscription derive reads a fallible return by its spelling,
/// so a `Result` under another name is refused here.
#[diagnostic::on_unimplemented(
    message = "a `#[subscription]` answers a stream, and this one returns a `Result` spelled \
               another way",
    label = "a `Result` under another name",
    note = "write it `Result<impl Stream<Item = T>, E>`: async-graphql reads a fallible return by \
            its spelling — `Result` or `FieldResult` — and takes any other for the stream itself"
)]
pub trait IsStreamReturn {}

impl IsStreamReturn for nest_rs_core::__private::ValueAnswer {}

/// Accept a `#[subscription]`'s answer — see [`IsStreamReturn`].
pub fn answers_a_stream<K: IsStreamReturn>(_: K) {}
