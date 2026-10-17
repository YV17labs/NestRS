//! [`TestClient`]: an app's endpoint driven in process, and the
//! [`TestRequestBuilder`] each request is written with.

// The client's method names and semantics are ported from poem 3.1.12
// (`src/test/client.rs`, `src/test/request_builder.rs`,
// https://github.com/poem-web/poem), under its MIT license, whose notice
// follows:
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::fmt;
use std::net::SocketAddr;

use nest_rs_http::{
    Body, BoxEndpoint, Endpoint, HeaderName, HeaderValue, HttpError, Method, Request,
    RequestBuilder, Uri, header,
};
use serde::Serialize;

use crate::form::TestForm;
use crate::response::TestResponse;
use crate::urlencoded;

/// Drives an app's assembled endpoint in process: no socket, so no peer
/// address unless a request sets one.
///
/// A request runs through the endpoint whole — for a
/// [`TestApp`](crate::TestApp)'s, the edge, the router, the guards and the
/// handler — and an error it answers with is rendered as the server renders
/// it.
///
/// ```
/// use nest_rs_http::{Response, endpoint_fn};
/// use nest_rs_testing::TestClient;
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// let client = TestClient::new(endpoint_fn(|_req| async { Ok(Response::builder().body("hello")) }));
///
/// let resp = client.get("/").send().await;
/// resp.assert_status_is_ok();
/// resp.assert_text("hello").await;
/// # }
/// ```
#[derive(Debug)]
pub struct TestClient {
    endpoint: BoxEndpoint,
}

impl TestClient {
    /// A client over `endpoint`.
    pub fn new(endpoint: impl Endpoint) -> Self {
        Self {
            endpoint: BoxEndpoint::new(endpoint),
        }
    }

    /// A request of `method` to `uri`, a path with its query when it has one.
    pub fn request(&self, method: Method, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        TestRequestBuilder {
            client: self,
            uri: uri.into(),
            query: String::new(),
            head: Request::builder().method(method),
            body: Body::empty(),
        }
    }

    /// A `GET` request to `uri`.
    pub fn get(&self, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        self.request(Method::GET, uri)
    }

    /// A `POST` request to `uri`.
    pub fn post(&self, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        self.request(Method::POST, uri)
    }

    /// A `PUT` request to `uri`.
    pub fn put(&self, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        self.request(Method::PUT, uri)
    }

    /// A `PATCH` request to `uri`.
    pub fn patch(&self, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        self.request(Method::PATCH, uri)
    }

    /// A `DELETE` request to `uri`.
    pub fn delete(&self, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        self.request(Method::DELETE, uri)
    }

    /// A `HEAD` request to `uri`.
    pub fn head(&self, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        self.request(Method::HEAD, uri)
    }

    /// An `OPTIONS` request to `uri`.
    pub fn options(&self, uri: impl Into<String>) -> TestRequestBuilder<'_> {
        self.request(Method::OPTIONS, uri)
    }
}

/// One request a [`TestClient`] sends: its headers, query, body, extensions
/// and peer, then [`send`](Self::send).
///
/// A malformed URI, header name or header value is a bug in the test, and
/// panics naming it.
pub struct TestRequestBuilder<'a> {
    client: &'a TestClient,
    uri: String,
    query: String,
    head: RequestBuilder,
    body: Body,
}

impl TestRequestBuilder<'_> {
    /// Appends the header `name: value`, keeping any value already set under
    /// that name.
    ///
    /// # Panics
    ///
    /// When `name` is not a header name, or `value` not a header value; the
    /// panic names the header, never the value, which may be a credential.
    ///
    /// ```
    /// # use nest_rs_http::{Response, endpoint_fn};
    /// # use nest_rs_testing::TestClient;
    /// # #[nest_rs_core::main]
    /// # async fn main() {
    /// # let client = TestClient::new(endpoint_fn(|req| async move {
    /// #     Ok(Response::builder().body(req.header("x-tenant").unwrap_or_default().to_owned()))
    /// # }));
    /// let resp = client.get("/").header("x-tenant", "acme").send().await;
    /// resp.assert_text("acme").await;
    /// # }
    /// ```
    #[must_use]
    pub fn header<V>(mut self, name: impl AsRef<str>, value: V) -> Self
    where
        V: TryInto<HeaderValue>,
    {
        let name = header_name(name.as_ref());
        let value = value.try_into().unwrap_or_else(|_| {
            panic!("the value given for the `{name}` header is not a valid header value")
        });
        self.head = self.head.header(name, value);
        self
    }

    /// Appends a `Content-Type` header.
    #[must_use]
    pub fn content_type(self, value: impl AsRef<str>) -> Self {
        self.header(header::CONTENT_TYPE, value.as_ref())
    }

    /// Appends `name=value` to the query string, form-encoded as a form field
    /// is: a string, a number, a boolean or a unit variant as serde writes it,
    /// and nothing for `None`.
    ///
    /// ```
    /// # use nest_rs_http::{Response, endpoint_fn};
    /// # use nest_rs_testing::TestClient;
    /// # #[nest_rs_core::main]
    /// # async fn main() {
    /// # let client = TestClient::new(endpoint_fn(|req| async move {
    /// #     Ok(Response::builder().body(req.uri().query().unwrap_or_default().to_owned()))
    /// # }));
    /// let resp = client.get("/posts").query("q", &"a b").query("page", &2).send().await;
    /// resp.assert_text("q=a+b&page=2").await;
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// When `value` is anything else, naming the parameter.
    #[must_use]
    pub fn query(mut self, name: impl AsRef<str>, value: &impl Serialize) -> Self {
        urlencoded::append_parameter(&mut self.query, name.as_ref(), value);
        self
    }

    /// Sets the body.
    #[must_use]
    pub fn body(mut self, body: impl Into<Body>) -> Self {
        self.body = body.into();
        self
    }

    /// Sets `value` as a JSON body, with `Content-Type: application/json`.
    ///
    /// # Panics
    ///
    /// When `value` does not serialize to JSON.
    #[must_use]
    pub fn body_json(self, value: &impl Serialize) -> Self {
        let json = serde_json::to_string(value)
            .unwrap_or_else(|error| panic!("the body does not serialize to JSON: {error}"));
        self.content_type("application/json").body(json)
    }

    /// Sets `value` — a struct, a map or a sequence of pairs — as an
    /// `application/x-www-form-urlencoded` body, its fields in the order they
    /// serialize; a field holding `None` is left out.
    ///
    /// ```
    /// # use nest_rs_http::{Response, endpoint_fn};
    /// # use nest_rs_testing::TestClient;
    /// # #[nest_rs_core::main]
    /// # async fn main() {
    /// # let client = TestClient::new(endpoint_fn(|req| async move {
    /// #     Ok(Response::builder().body(req.into_body()))
    /// # }));
    /// let resp = client.post("/login").form(&[("email", "ada@example.com")]).send().await;
    /// resp.assert_text("email=ada%40example.com").await;
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// When `value` is none of those, or a field holds anything but a string,
    /// a number, a boolean, a unit variant or an `Option` of one, naming the
    /// field.
    #[must_use]
    pub fn form(self, value: &impl Serialize) -> Self {
        self.content_type("application/x-www-form-urlencoded")
            .body(urlencoded::encode_form(value))
    }

    /// Sets `form` as a `multipart/form-data` body.
    #[must_use]
    pub fn multipart(self, form: TestForm) -> Self {
        self.content_type(format!(
            "multipart/form-data; boundary={}",
            TestForm::BOUNDARY
        ))
        .body(form.into_bytes())
    }

    /// Adds `value` to the request's extensions, where a handler, a guard or a
    /// layer reads it by type.
    #[must_use]
    pub fn data<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.head = self.head.extension(value);
        self
    }

    /// Sends the request as if from `addr`: what [`ClientIp`] and the
    /// trusted-proxy check read. In process a request has no peer otherwise.
    ///
    /// [`ClientIp`]: nest_rs_http::ClientIp
    #[must_use]
    pub fn peer_addr(mut self, addr: SocketAddr) -> Self {
        self.head = self.head.peer_addr(addr);
        self
    }

    /// Sends the request through the client's endpoint and hands back its
    /// response; an error the endpoint answers is rendered as the server
    /// renders it.
    ///
    /// # Panics
    ///
    /// When the URI, with the query appended, is not a request URI.
    pub async fn send(self) -> TestResponse {
        let Self {
            client,
            mut uri,
            query,
            head,
            body,
        } = self;
        if !query.is_empty() {
            uri.push(if uri.contains('?') { '&' } else { '?' });
            uri.push_str(&query);
        }
        let parsed = uri
            .parse::<Uri>()
            .unwrap_or_else(|error| panic!("`{uri}` is not a request URI: {error}"));
        let request = head.uri(parsed).body(body);
        let response = client
            .endpoint
            .call(request)
            .await
            .unwrap_or_else(HttpError::into_response);
        TestResponse(response)
    }
}

impl fmt::Debug for TestRequestBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestRequestBuilder")
            .field("head", &self.head)
            .field("uri", &self.uri)
            .finish_non_exhaustive()
    }
}

/// `name` as a header name.
///
/// # Panics
///
/// Naming it, when it is not one.
pub(crate) fn header_name(name: &str) -> HeaderName {
    HeaderName::from_bytes(name.as_bytes())
        .unwrap_or_else(|_| panic!("{name:?} is not a valid header name"))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::panic::AssertUnwindSafe;

    use nest_rs_http::{Response, endpoint_fn};

    use super::*;

    /// A client whose endpoint answers with what `read` reads off the request.
    fn echoing(read: fn(&Request) -> String) -> TestClient {
        TestClient::new(endpoint_fn(move |req: Request| async move {
            Ok(Response::builder().body(read(&req)))
        }))
    }

    #[test]
    #[should_panic(expected = "\"x tenant\" is not a valid header name")]
    fn an_invalid_test_header_panics_naming_it() {
        let client = echoing(|_| String::new());
        drop(client.get("/").header("x tenant", "acme"));
    }

    #[test]
    fn an_invalid_header_value_panics_naming_its_header_and_not_the_value() {
        let client = echoing(|_| String::new());
        let panic = std::panic::catch_unwind(AssertUnwindSafe(|| {
            drop(
                client
                    .get("/")
                    .header(header::AUTHORIZATION, "Bearer secret\r\n"),
            );
        }))
        .expect_err("a malformed header value panics");
        let message = panic.downcast_ref::<String>().expect("a formatted panic");
        assert!(message.contains("`authorization` header"), "{message}");
        assert!(!message.contains("secret"), "{message}");
    }

    #[tokio::test]
    async fn the_default_peer_is_none_and_peer_addr_sets_one() {
        let client = echoing(|req| format!("{:?}", req.peer_addr()));

        client.get("/").send().await.assert_text("None").await;

        let peer: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        client
            .get("/")
            .peer_addr(peer)
            .send()
            .await
            .assert_text(format!("Some({peer})"))
            .await;
    }

    #[tokio::test]
    async fn an_error_the_endpoint_answers_is_rendered_as_the_server_renders_it() {
        let client = TestClient::new(endpoint_fn(|_req| async {
            Err(HttpError::from_status(nest_rs_http::StatusCode::CONFLICT))
        }));
        let resp = client.get("/").send().await;
        resp.assert_status(nest_rs_http::StatusCode::CONFLICT);
        resp.assert_content_type("application/problem+json");
    }

    #[tokio::test]
    async fn the_query_is_appended_as_serde_writes_each_value() {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum Sort {
            NewestFirst,
        }
        let client = echoing(|req| req.uri().to_string());
        client
            .get("/search?scope=all")
            .query("q", &"café & co")
            .query(String::from("sort"), &Sort::NewestFirst)
            .query("cursor", &None::<u32>)
            .query("page", &Some(2))
            .send()
            .await
            .assert_text("/search?scope=all&q=caf%C3%A9+%26+co&sort=newest_first&page=2")
            .await;
    }

    #[tokio::test]
    async fn a_form_is_sent_in_declaration_order_with_its_content_type() {
        #[derive(Serialize)]
        struct Token {
            grant_type: &'static str,
            code: &'static str,
        }
        let client = TestClient::new(endpoint_fn(|req: Request| async move {
            let content_type = req
                .header(header::CONTENT_TYPE)
                .unwrap_or_default()
                .to_owned();
            let body = req.into_body().into_string().await.unwrap();
            Ok(Response::builder().body(format!("{content_type} {body}")))
        }));
        client
            .post("/token")
            .form(&Token {
                grant_type: "authorization_code",
                code: "abc",
            })
            .send()
            .await
            .assert_text("application/x-www-form-urlencoded grant_type=authorization_code&code=abc")
            .await;
    }

    #[tokio::test]
    async fn headers_append_and_data_rides_the_extensions() {
        let client = echoing(|req| {
            let accept: Vec<_> = req
                .headers()
                .get_all(header::ACCEPT)
                .iter()
                .map(|value| value.to_str().unwrap())
                .collect();
            format!("{} {:?}", accept.join(","), req.extensions().get::<u32>())
        });
        client
            .get("/")
            .header(header::ACCEPT, "text/html")
            .header("accept", "*/*")
            .data(7_u32)
            .send()
            .await
            .assert_text("text/html,*/* Some(7)")
            .await;
    }
}
