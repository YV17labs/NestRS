//! [`GetEndpoint`] — what `GET` on the GraphQL path answers.

use nest_rs_http::__private::{method_not_allowed, websocket};
use nest_rs_http::{Bytes, Endpoint, HeaderValue, HttpError, Request, Response};

use crate::subscription::SubscriptionEndpoint;

/// `GET <path>`: the graphql-ws socket, or the playground — one path, since
/// graphql-ws clients point their socket at the URL they POST to.
pub(crate) struct GetEndpoint<E> {
    subscriptions: SubscriptionEndpoint<E>,
    /// Rendered once at mount; `None` when the playground is closed.
    playground: Option<Bytes>,
}

impl<E> GetEndpoint<E> {
    pub(crate) fn new(subscriptions: SubscriptionEndpoint<E>, playground: Option<String>) -> Self {
        Self {
            subscriptions,
            playground: playground.map(Bytes::from),
        }
    }
}

impl<E: async_graphql::Executor> Endpoint for GetEndpoint<E> {
    async fn call(&self, req: Request) -> nest_rs_http::Result<Response> {
        if websocket::requested(req.headers()) {
            return self.subscriptions.call(req).await;
        }
        match &self.playground {
            Some(html) => Ok(Response::builder()
                .content_type(HeaderValue::from_static("text/html; charset=utf-8"))
                .body(html.clone())),
            None => Err(HttpError::from_response(method_not_allowed(
                HeaderValue::from_static("POST"),
            ))),
        }
    }
}
