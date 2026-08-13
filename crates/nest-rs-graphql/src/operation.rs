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
