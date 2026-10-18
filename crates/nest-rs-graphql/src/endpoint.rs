//! [`GetEndpoint`] — what `GET` on the GraphQL path answers.

use nest_rs_http::__private::poem_bridge::{error_to_poem, request_from_poem, response_to_poem};
use nest_rs_http::__private::websocket;
use poem::{Endpoint, IntoResponse, Request, Response};

use crate::subscription::SubscriptionEndpoint;

/// `GET <path>`: the graphql-ws socket, or the playground — one path, since
/// graphql-ws clients point their socket at the URL they POST to.
pub(crate) struct GetEndpoint<E> {
    subscriptions: SubscriptionEndpoint<E>,
    /// Rendered once at mount; `None` when the playground is off (the default).
    playground: Option<String>,
}

impl<E> GetEndpoint<E> {
    pub(crate) fn new(subscriptions: SubscriptionEndpoint<E>, playground: Option<String>) -> Self {
        Self {
            subscriptions,
            playground,
        }
    }
}

impl<E: async_graphql::Executor> Endpoint for GetEndpoint<E> {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        if websocket::requested(req.headers()) {
            return nest_rs_http::Endpoint::call(&self.subscriptions, request_from_poem(req))
                .await
                .map(response_to_poem)
                .map_err(error_to_poem);
        }
        match &self.playground {
            Some(html) => Ok(poem::web::Html(html.clone()).into_response()),
            None => Err(poem::Error::from_status(
                poem::http::StatusCode::METHOD_NOT_ALLOWED,
            )),
        }
    }
}
