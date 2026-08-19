//! `Allow` — the method set a route table serves, carried from the declaration
//! that states it to the `405` that has to name it.
//!
//! [RFC 9110 §15.5.6](https://www.rfc-editor.org/rfc/rfc9110#status.405) is a
//! **MUST**: "The origin server MUST generate an Allow header field in a 405
//! response containing a list of the target resource's currently supported
//! methods." Without it a client is told its method is wrong and nothing about
//! which one is right — the one piece of information the status exists to
//! deliver.
//!
//! The set is known where the routes are declared: `#[routes]` groups its verbs
//! by path to collapse them into one method table. It was computed there and
//! discarded at the mount, so poem's `MethodNotAllowedError` rendered a bare
//! status with nothing to put in the header. [`MethodTable`] is what carries it
//! through — the same call that registers a verb records it, so the served set
//! and the advertised set cannot drift.
//!
//! Two methods are advertised exactly as far as the router serves them:
//!
//! - **`HEAD`**, whenever `GET` is registered. poem's `RouteMethod` answers an
//!   unregistered `HEAD` by re-dispatching to the `GET` endpoint and dropping
//!   the body, which is what RFC 9110 §9.3.2 describes, so the method genuinely
//!   is supported at that resource.
//! - **`OPTIONS`, never.** Nothing under this transport registers one: an
//!   `OPTIONS` that reached a method table would itself answer `405`, and an
//!   advertised method that refuses the request is the lie this header exists to
//!   prevent. (A CORS preflight is answered by the CORS middleware, outside
//!   routing, and only for a request carrying `Origin` +
//!   `Access-Control-Request-Method`.) Serving `OPTIONS` with an `Allow` of its
//!   own — RFC 9110 §9.3.7, a SHOULD — would put an unguarded method-listing
//!   endpoint at every route, so it is an owner question rather than a member
//!   this module refuses.
//!
//! **The other mount shape is reported, not reached into.** A self-mounted
//! endpoint that routes by method builds its own table: `nest-rs-graphql` mounts
//! `poem::post(…).get(…)`, so a `PUT /graphql` still answers a bare `405`.
//! [`MethodTable`] is public for exactly that — it is a drop-in for poem's
//! `RouteMethod`, and the crate that owns a mount is the one that can name its
//! verbs.

use poem::error::MethodNotAllowedError;
use poem::http::{HeaderValue, Method, header};
use poem::{Endpoint, IntoEndpoint, Request, Response, Result, RouteMethod};

/// A [`RouteMethod`] that remembers which verbs were registered on it, so the
/// `405` it answers can carry `Allow` (RFC 9110 §15.5.6).
///
/// Built by `#[routes]` at mount time, one per path. Recording happens in the
/// same call that registers the endpoint — there is no second list to keep in
/// step.
pub struct MethodTable {
    inner: RouteMethod,
    allowed: Vec<Method>,
}

impl Default for MethodTable {
    fn default() -> Self {
        Self::new()
    }
}

impl MethodTable {
    /// An empty table — no verb registered, nothing advertised.
    pub fn new() -> Self {
        Self {
            inner: RouteMethod::new(),
            allowed: Vec::new(),
        }
    }

    /// Whether no verb has been registered. A path whose every route was
    /// narrowed out by `#[version]` claims no address at all: an empty table
    /// mounted at a path answers `405`, which is a worse lie than the `404` the
    /// router gives when nothing claims it.
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }

    /// Register `ep` for `method`, recording it in the advertised set.
    pub fn method<E>(mut self, method: Method, ep: E) -> Self
    where
        E: IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.allowed.push(method.clone());
        self.inner = self.inner.method(method, ep);
        self
    }

    /// Register `ep` for `GET` — which also makes `HEAD` supported, since poem
    /// answers an unregistered `HEAD` through the `GET` endpoint.
    pub fn get<E>(self, ep: E) -> Self
    where
        E: IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.method(Method::GET, ep)
    }

    /// Register `ep` for `POST`.
    pub fn post<E>(self, ep: E) -> Self
    where
        E: IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.method(Method::POST, ep)
    }

    /// Register `ep` for `PUT`.
    pub fn put<E>(self, ep: E) -> Self
    where
        E: IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.method(Method::PUT, ep)
    }

    /// Register `ep` for `DELETE`.
    pub fn delete<E>(self, ep: E) -> Self
    where
        E: IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.method(Method::DELETE, ep)
    }

    /// Register `ep` for `PATCH`.
    pub fn patch<E>(self, ep: E) -> Self
    where
        E: IntoEndpoint,
        E::Endpoint: 'static,
    {
        self.method(Method::PATCH, ep)
    }

    /// The `Allow` field value this table advertises: the registered verbs in
    /// declaration order, with `HEAD` beside the `GET` that implies it.
    pub fn allow_value(&self) -> String {
        let mut methods: Vec<&str> = Vec::with_capacity(self.allowed.len() + 1);
        for method in &self.allowed {
            methods.push(method.as_str());
            if method == Method::GET && !self.allowed.contains(&Method::HEAD) {
                methods.push(Method::HEAD.as_str());
            }
        }
        methods.join(", ")
    }

    /// The endpoint to mount: the method table, plus the `Allow` header on the
    /// `405` it answers.
    pub fn into_endpoint(self) -> AllowedMethods {
        // Every `Method::as_str()` is an RFC 9110 token and `", "` is a legal
        // separator between two of them, so this conversion cannot fail — the
        // `Option` is the type system's, not a policy. On the branch that
        // cannot be taken the `405` is simply the one poem already rendered,
        // which is what this module found rather than something it introduced.
        let allow = HeaderValue::try_from(self.allow_value()).ok();
        AllowedMethods {
            inner: self.inner,
            allow,
        }
    }
}

/// A method table whose `405` carries `Allow` (RFC 9110 §15.5.6). Built by
/// [`MethodTable::into_endpoint`].
pub struct AllowedMethods {
    inner: RouteMethod,
    allow: Option<HeaderValue>,
}

impl Endpoint for AllowedMethods {
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        let result = self.inner.call(req).await;
        // Narrowed to poem's own routing error rather than to "any `405`": a
        // handler that deliberately answers `405` is stating something about
        // its own resource, and turning its `Err` into an `Ok` here would also
        // take it out of the rollback path a mapped error travels.
        match result {
            Err(err) if err.is::<MethodNotAllowedError>() => {
                let mut resp = err.into_response();
                if let Some(allow) = self.allow.clone() {
                    resp.headers_mut().insert(header::ALLOW, allow);
                }
                Ok(resp)
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem::handler;
    use poem::test::TestClient;

    #[handler]
    fn ok() -> &'static str {
        "ok"
    }

    #[test]
    fn an_empty_table_advertises_nothing_and_claims_no_path() {
        let table = MethodTable::new();
        assert!(table.is_empty());
        assert_eq!(table.allow_value(), "");
    }

    #[test]
    fn a_get_carries_the_head_poem_answers_through_it() {
        assert_eq!(MethodTable::new().get(ok).allow_value(), "GET, HEAD");
    }

    #[test]
    fn verbs_are_advertised_in_declaration_order() {
        let table = MethodTable::new().post(ok).get(ok).delete(ok);
        assert_eq!(table.allow_value(), "POST, GET, HEAD, DELETE");
    }

    #[test]
    fn a_table_without_a_get_advertises_no_head() {
        assert_eq!(
            MethodTable::new().post(ok).patch(ok).put(ok).allow_value(),
            "POST, PATCH, PUT",
        );
    }

    #[tokio::test]
    async fn an_unsupported_method_is_answered_with_the_allow_header() {
        let ep = MethodTable::new().get(ok).post(ok).into_endpoint();
        let resp = TestClient::new(ep).delete("/").send().await;
        resp.assert_status(poem::http::StatusCode::METHOD_NOT_ALLOWED);
        resp.assert_header(header::ALLOW, "GET, HEAD, POST");
    }

    // The advertised `HEAD` is poem's `GET` fallback, so it has to answer —
    // advertising a method the router refuses is what this module exists to
    // stop.
    #[tokio::test]
    async fn the_advertised_head_is_served_by_the_get_endpoint() {
        let ep = MethodTable::new().get(ok).into_endpoint();
        let resp = TestClient::new(ep).head("/").send().await;
        resp.assert_status_is_ok();
    }

    #[tokio::test]
    async fn a_registered_method_passes_through_untouched() {
        let ep = MethodTable::new().get(ok).into_endpoint();
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        assert!(
            resp.0.headers().get(header::ALLOW).is_none(),
            "`Allow` belongs on the refusal, not on every response",
        );
    }
}
