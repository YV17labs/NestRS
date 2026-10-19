//! [`GetEndpoint`] — what `GET` on the GraphQL path answers.

use nest_rs_http::__private::websocket;
use nest_rs_http::{
    Body, Bytes, Endpoint, HeaderValue, HttpError, IntoResponse, ProblemDetails, Request, Response,
    StatusCode, header,
};

use crate::subscription::SubscriptionEndpoint;

/// `GET <path>`: the graphql-ws socket, or the playground — one path, since
/// graphql-ws clients point their socket at the URL they POST to.
pub(crate) struct GetEndpoint<E> {
    subscriptions: SubscriptionEndpoint<E>,
    /// Rendered once at mount; `None` when the playground is off (the default).
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
                .body(Body::from_bytes(html.clone()))),
            None => Err(HttpError::from_response(not_a_graphql_request())),
        }
    }
}

/// A plain `GET` with the playground off: `405` naming the method a GraphQL
/// request takes here (RFC 9110 §15.5.6), `GET` serving the socket alone.
fn not_a_graphql_request() -> Response {
    let mut refused = ProblemDetails::from_status(StatusCode::METHOD_NOT_ALLOWED).into_response();
    refused
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    refused
}
