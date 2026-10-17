//! [`GetEndpoint`] — what `GET` on the GraphQL path answers.

use poem::http::header;
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
        // `Upgrade` decides, not `Connection`, a hop-by-hop header a proxy may rewrite.
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
            None => Err(poem::Error::from_status(
                poem::http::StatusCode::METHOD_NOT_ALLOWED,
            )),
        }
    }
}
