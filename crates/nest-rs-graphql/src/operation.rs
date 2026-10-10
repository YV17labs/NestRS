//! What one GraphQL operation *is*, for the layers that run around it.
//!
//! The federation root fields are reached only through an
//! [`Extension`](async_graphql::extensions::Extension), handed an
//! [`ExtensionContext`] and never a `Context`, with no public constructor to
//! bridge the two. [`GraphqlOperationContext`] spans both sites, so
//! `check_graphql` is declared once.

use std::any::Any;

use async_graphql::extensions::ExtensionContext;
use async_graphql::{Context, Result};
use tracing::Instrument;

/// One GraphQL operation, as a [`Guard`](https://docs.rs/nest-rs-guards) sees
/// it at a resolver field or a federation root field.
pub struct GraphqlOperationContext<'a> {
    site: Site<'a>,
}

/// The two places async-graphql lets a check run.
enum Site<'a> {
    /// A resolver field — `#[query]`, `#[mutation]`, `#[entity]`,
    /// `#[field_resolver]`. Emitted inline by `#[operations]`.
    Field(&'a Context<'a>),
    /// A federation root field, from the schema extension; it belongs to no
    /// resolver.
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
    pub(crate) fn federation(ctx: &'a ExtensionContext<'a>, field: &'static str) -> Self {
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

    /// The full async-graphql context — arguments, selection set — or `None`
    /// on a federation root field, which has none.
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

/// Run one dispatched GraphQL field as its own unit of work; `#[operations]`
/// wraps every `#[query]`, `#[mutation]`, `#[entity]` and `#[field_resolver]`
/// body in this.
///
/// `operation` is the field name **as the client wrote it** (`listUsers`, not
/// `list_users`), so the line joins against a capture of the request.
///
/// The line files `ok` or `error` on return, `cancelled` when dropped first, and
/// `panic` when it unwinds — caught only to be named, then resumed. A sibling
/// torn down by that unwind files `cancelled`: one panic is one `panic` line.
pub async fn run_operation<T, F>(
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
        crate::unit::OPERATION,
        &correlation,
        // Not OTel's `graphql.operation.type`: `role` adds `entity` and
        // `field_resolver` to its enum. `graphql.document` is omitted: it carries
        // the caller's literals.
        graphql.operation.role = role,
        graphql.field.name = operation,
    );
    let line = OperationLine {
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

/// One field's `graphql.operation` line, filed exactly once, by `Drop` as
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED) when dropped first.
///
/// The correlation is held: a `Drop` runs while the future is torn down, not
/// reliably inside the scope that future installed.
struct OperationLine<'a> {
    role: &'static str,
    operation: &'a str,
    correlation: nest_rs_core::Correlation,
    span: tracing::Span,
    started: std::time::Instant,
    filed: bool,
}

impl OperationLine<'_> {
    fn file(mut self, outcome: &'static str) {
        self.emit(outcome);
    }

    fn emit(&mut self, outcome: &'static str) {
        self.filed = true;
        nest_rs_core::RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            nest_rs_core::operation_line!(
                crate::unit::OPERATION,
                span: &self.span,
                outcome: outcome,
                started: self.started,
                role = self.role,
                operation = self.operation,
            );
        });
    }
}

impl Drop for OperationLine<'_> {
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
