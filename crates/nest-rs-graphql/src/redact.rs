//! Every error a GraphQL response carries, said without a decode failure's
//! value.
//!
//! async-graphql builds an error's `message` from the failure's `Display`, and
//! serde's sentence quotes the value; each message is said through
//! [`DecodeError::redact`].
//!
//! An [`Executor`] wrapper rather than an extension, which would be entered on
//! every field of every request.

use std::sync::Arc;

use async_graphql::futures_util::StreamExt;
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::{Data, Executor, Request, Response, ServerError};
use nest_rs_core::DecodeError;

/// The schema, answering with every error message redacted; a batch goes
/// through [`execute`](Executor::execute), the trait's default.
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

/// One error's message, redacted against its source when it is serde's or an
/// anyhow chain; any other source is read by serde's wording alone.
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
