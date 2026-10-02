//! Every error a GraphQL response carries, said without a decode failure's
//! value.
//!
//! A GraphQL error's `message` is the client's to read, and async-graphql builds
//! it from whatever the failure displays: a resolver's `?` on a serde or anyhow
//! error goes through its `From<T: Display>`, and its own `Json<T>` scalar
//! coerces an argument with serde and reports `Failed to parse "JSON": <serde's
//! sentence>`. serde's sentence quotes the value — a variable the client sent, or
//! a record the resolver read — so the edge says each message the way every
//! other edge says a reply ([`DecodeError::redact`]).
//!
//! An [`Executor`] wrapper rather than an async-graphql extension: an extension
//! is entered on every field of every request, and this has nothing to do until
//! a response carries an error. The mount wraps the schema once, which puts
//! the POST path and the WebSocket path behind it alike, and
//! [`compose_schema`](crate::compose_schema) — the executor a subscriber's
//! witness drives — wraps it the same way.

use std::sync::Arc;

use async_graphql::futures_util::StreamExt;
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::{Data, Executor, Request, Response, ServerError};
use nest_rs_core::DecodeError;

/// The schema, answering with every error message redacted. A batch is
/// executed request by request through [`execute`](Executor::execute), the
/// trait's own default, so it is redacted the same way.
#[derive(Clone)]
pub(crate) struct Redacted<E>(pub(crate) E);

impl<E: Executor> Executor for Redacted<E> {
    async fn execute(&self, request: Request) -> Response {
        redacted(self.0.execute(request).await)
    }

    fn execute_stream(
        &self,
        request: Request,
        session_data: Option<Arc<Data>>,
    ) -> BoxStream<'static, Response> {
        self.0
            .execute_stream(request, session_data)
            .map(redacted)
            .boxed()
    }
}

fn redacted(mut response: Response) -> Response {
    response.errors.iter_mut().for_each(redact);
    response
}

/// One error's message, redacted against its source when async-graphql kept one
/// the framework can read as an error: serde's own, or an anyhow chain. Any
/// other source is `dyn Any` to the framework, and the message is read by
/// serde's wording alone.
fn redact(error: &mut ServerError) {
    let source = error.source.as_deref();
    let cause: Option<&(dyn std::error::Error + 'static)> = source
        .and_then(|source| source.downcast_ref::<serde_json::Error>())
        .map(|error| error as &(dyn std::error::Error + 'static))
        .or_else(|| {
            source
                .and_then(|source| source.downcast_ref::<nest_rs_core::anyhow::Error>())
                .map(|error| error.as_ref() as &(dyn std::error::Error + 'static))
        });
    if let std::borrow::Cow::Owned(message) = DecodeError::redact(&error.message, cause) {
        error.message = message;
    }
}
