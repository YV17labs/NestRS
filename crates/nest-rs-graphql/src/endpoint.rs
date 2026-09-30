//! [`GetEndpoint`] — what `GET` on the GraphQL path answers.
//!
//! Beside the POST half ([`ContextEndpoint`](crate::context::ContextEndpoint))
//! rather than in `module.rs`, which holds the DI module and the types that
//! share its stem, and nothing else.

use poem::http::header;
use poem::{Endpoint, IntoResponse, Request, Response};

use crate::subscription::SubscriptionEndpoint;

/// `GET <path>`: the graphql-ws socket, or the playground.
///
/// One path rather than two, because that is what a graphql-ws client assumes —
/// `graphql-ws`, Apollo and the playground all point their socket at the same
/// URL they POST to. The two are told apart by the request, not by the route:
/// an upgrade is a subscription, anything else is a browser.
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
        // `Connection: Upgrade` is the hop-by-hop header a proxy may rewrite or
        // list beside other tokens, so the *presence of an upgrade target*
        // (`Upgrade: websocket`) is what decides — the same fact poem's own
        // `WebSocket` extractor requires, checked before we take the socket path
        // so a plain browser still gets the playground.
        let upgrading = req
            .headers()
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
        if upgrading {
            return self.subscriptions.call(req).await;
        }
        match &self.playground {
            Some(html) => Ok(poem::web::Html(html.clone()).into_response()),
            // No playground and not an upgrade: the path serves POST and
            // sockets, so a bare GET is the wrong method, not a missing route.
            None => Err(poem::Error::from_status(
                poem::http::StatusCode::METHOD_NOT_ALLOWED,
            )),
        }
    }
}
