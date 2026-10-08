//! `Allow` — the method set a route table serves, carried to the `405` that
//! must name it ([RFC 9110 §15.5.6](https://www.rfc-editor.org/rfc/rfc9110#status.405)).
//!
//! `HEAD` is advertised whenever `GET` is registered, since poem answers it
//! through the `GET` endpoint; `OPTIONS` never, since nothing here serves it. A
//! self-mount routing by method builds its own table: `PUT /graphql` still
//! answers a bare `405`.

use poem::error::MethodNotAllowedError;
use poem::http::{HeaderValue, Method, header};
use poem::{Endpoint, IntoEndpoint, Request, Response, Result, RouteMethod};

/// A [`RouteMethod`] that remembers which verbs were registered on it, so the
/// `405` it answers can carry `Allow` (RFC 9110 §15.5.6). Built by `#[routes]`
/// at mount time, one per path.
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

    /// Whether no verb has been registered — such a table must not be mounted,
    /// or its path answers `405` where it should `404`.
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
        // Cannot fail: every `Method::as_str()` is an RFC 9110 token.
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
        // Every controller route passes here, so this is where it notes its route.
        crate::matched::note(&req);
        let result = self.inner.call(req).await;
        // poem's routing error only: a handler's own `405` `Err` must keep its
        // rollback path.
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
