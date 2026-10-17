//! [`TestResponse`], what a [`TestClient`](crate::TestClient) hands back, and
//! [`TestJson`], its body read as JSON.

// The assertions' names and semantics are ported from poem 3.1.12
// (`src/test/response.rs`, `src/test/json.rs`,
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

use nest_rs_http::{Bytes, HeaderValue, Response, StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::client::header_name;

/// A response a [`TestClient`](crate::TestClient) received, with assertions
/// whose failure names what differed.
///
/// The response itself is `.0`, for what no assertion reads.
///
/// ```
/// use nest_rs_http::{Response, StatusCode, endpoint_fn};
/// use nest_rs_testing::TestClient;
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// let client = TestClient::new(endpoint_fn(|_req| async {
///     Ok(Response::builder().status(StatusCode::CREATED).body("made"))
/// }));
///
/// let resp = client.post("/orders").send().await;
/// resp.assert_status(StatusCode::CREATED);
/// assert!(resp.0.status().is_success());
/// resp.assert_text("made").await;
/// # }
/// ```
#[derive(Debug)]
pub struct TestResponse(pub Response);

impl TestResponse {
    /// Asserts the status is `status`.
    #[track_caller]
    pub fn assert_status(&self, status: StatusCode) {
        assert_eq!(self.0.status(), status, "the response status");
    }

    /// Asserts the status is `200 OK`.
    #[track_caller]
    pub fn assert_status_is_ok(&self) {
        self.assert_status(StatusCode::OK);
    }

    /// Asserts the header `name` is set, its first value `value`.
    ///
    /// # Panics
    ///
    /// When the header is missing or holds another value, naming it; or when
    /// `name` or `value` cannot be a header's.
    #[track_caller]
    pub fn assert_header<V>(&self, name: impl AsRef<str>, value: V)
    where
        V: TryInto<HeaderValue>,
    {
        let name = header_name(name.as_ref());
        let expected = expected_value(&name, value);
        let actual = self
            .0
            .headers()
            .get(&name)
            .unwrap_or_else(|| panic!("the response has no `{name}` header"));
        assert_eq!(actual, expected, "the `{name}` header");
    }

    /// Asserts the header `name` holds exactly `values`, in any order.
    #[track_caller]
    pub fn assert_header_all<V, I>(&self, name: impl AsRef<str>, values: I)
    where
        V: TryInto<HeaderValue>,
        I: IntoIterator<Item = V>,
    {
        let name = header_name(name.as_ref());
        let mut expected: Vec<HeaderValue> = values
            .into_iter()
            .map(|value| expected_value(&name, value))
            .collect();
        let mut actual: Vec<HeaderValue> =
            self.0.headers().get_all(&name).iter().cloned().collect();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected, "the `{name}` header's values");
    }

    /// Asserts the header `name` is set.
    #[track_caller]
    pub fn assert_header_exist(&self, name: impl AsRef<str>) {
        let name = header_name(name.as_ref());
        assert!(
            self.0.headers().contains_key(&name),
            "the response has no `{name}` header"
        );
    }

    /// Asserts the header `name` is not set.
    #[track_caller]
    pub fn assert_header_is_not_exist(&self, name: impl AsRef<str>) {
        let name = header_name(name.as_ref());
        assert!(
            !self.0.headers().contains_key(&name),
            "the response has a `{name}` header"
        );
    }

    /// Asserts the `Content-Type` is `value`.
    #[track_caller]
    pub fn assert_content_type(&self, value: &str) {
        self.assert_header(header::CONTENT_TYPE, value);
    }

    /// Asserts the body is the UTF-8 text `text`.
    pub async fn assert_text(self, text: impl AsRef<str>) {
        let body = self.bytes().await;
        let body = std::str::from_utf8(&body)
            .unwrap_or_else(|error| panic!("the response body is not UTF-8: {error}"));
        assert_eq!(body, text.as_ref(), "the response body");
    }

    /// Asserts the body is exactly `bytes`.
    pub async fn assert_bytes(self, bytes: impl AsRef<[u8]>) {
        assert_eq!(&self.bytes().await[..], bytes.as_ref(), "the response body");
    }

    /// Asserts the body is JSON equal to `value`'s.
    pub async fn assert_json(self, value: impl Serialize) {
        let expected = serde_json::to_value(value)
            .unwrap_or_else(|error| panic!("the expected value is not JSON: {error}"));
        assert_eq!(self.json().await.0, expected, "the response body");
    }

    /// The body, read as JSON.
    ///
    /// # Panics
    ///
    /// When it cannot be read, or is not JSON.
    pub async fn json(self) -> TestJson {
        let body = self.bytes().await;
        serde_json::from_slice(&body)
            .map(TestJson)
            .unwrap_or_else(|error| panic!("the response body is not JSON: {error}"))
    }

    /// Every byte of the body.
    async fn bytes(self) -> Bytes {
        self.0
            .into_body()
            .into_bytes()
            .await
            .unwrap_or_else(|error| panic!("the response body could not be read: {error}"))
    }
}

/// `value` as the value the header `name` is expected to hold.
fn expected_value<V: TryInto<HeaderValue>>(
    name: &nest_rs_http::HeaderName,
    value: V,
) -> HeaderValue {
    value.try_into().unwrap_or_else(|_| {
        panic!("the value expected of the `{name}` header is not a header value")
    })
}

/// A response body read as JSON, walked from [`value`](Self::value).
///
/// Each step that does not find what it reads panics naming the JSON path it
/// stood at, as `$.data.items[2]`.
///
/// ```
/// use nest_rs_http::{Response, endpoint_fn};
/// use nest_rs_testing::TestClient;
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// let client = TestClient::new(endpoint_fn(|_req| async {
///     Ok(Response::builder().body(r#"{"items":[{"id":7,"title":"Hello"}]}"#))
/// }));
///
/// let json = client.get("/posts").send().await.json().await;
/// let first = json.value().object().get("items").array().get(0).object();
/// assert_eq!(first.get("id").i64(), 7);
/// first.get("title").assert_string("Hello");
/// # }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TestJson(Value);

impl TestJson {
    /// The document's root.
    pub fn value(&self) -> TestJsonValue<'_> {
        TestJsonValue {
            root: &self.0,
            value: &self.0,
        }
    }
}

/// The document as it was read, for an assertion no step reads:
/// `serde_json::to_value(resp.json().await)`.
impl Serialize for TestJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// One value of a [`TestJson`] document.
#[derive(Clone, Copy)]
pub struct TestJsonValue<'a> {
    root: &'a Value,
    value: &'a Value,
}

impl<'a> TestJsonValue<'a> {
    /// The value, an object.
    #[track_caller]
    pub fn object(&self) -> TestJsonObject<'a> {
        match self.value {
            Value::Object(fields) => TestJsonObject {
                root: self.root,
                fields,
            },
            other => self.mismatch("an object", other),
        }
    }

    /// The value, an array.
    #[track_caller]
    pub fn array(&self) -> TestJsonArray<'a> {
        match self.value {
            Value::Array(items) => TestJsonArray {
                root: self.root,
                items,
            },
            other => self.mismatch("an array", other),
        }
    }

    /// The value, a string.
    #[track_caller]
    pub fn string(&self) -> &'a str {
        match self.value {
            Value::String(text) => text,
            other => self.mismatch("a string", other),
        }
    }

    /// The value, an integer that fits an `i64`.
    #[track_caller]
    pub fn i64(&self) -> i64 {
        self.value
            .as_i64()
            .unwrap_or_else(|| self.mismatch("an i64", self.value))
    }

    /// Asserts the value is the string `text`.
    #[track_caller]
    pub fn assert_string(&self, text: &str) {
        assert_eq!(self.string(), text, "`{}`", self.path());
    }

    /// The value, deserialized into `T`.
    #[track_caller]
    pub fn deserialize<T: DeserializeOwned>(&self) -> T {
        T::deserialize(self.value).unwrap_or_else(|error| {
            panic!(
                "`{}` does not deserialize into {}: {error}",
                self.path(),
                std::any::type_name::<T>(),
            )
        })
    }

    /// Where this value sits in its document, from `$`.
    fn path(&self) -> String {
        path_to(self.root, self.value)
    }

    #[track_caller]
    fn mismatch(&self, expected: &str, actual: &Value) -> ! {
        panic!("`{}` is {}, not {expected}", self.path(), kind(actual))
    }
}

impl fmt::Debug for TestJsonValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.value, f)
    }
}

/// An object of a [`TestJson`] document.
#[derive(Clone, Copy)]
pub struct TestJsonObject<'a> {
    root: &'a Value,
    fields: &'a Map<String, Value>,
}

impl<'a> TestJsonObject<'a> {
    /// The value under `name`.
    ///
    /// # Panics
    ///
    /// When the object has no `name`, naming the object's path.
    #[track_caller]
    pub fn get(&self, name: impl AsRef<str>) -> TestJsonValue<'a> {
        let name = name.as_ref();
        self.get_opt(name).unwrap_or_else(|| {
            panic!(
                "`{}` has no key `{name}`",
                path_to(self.root, self.as_value())
            )
        })
    }

    /// The value under `name`, when the object has one.
    pub fn get_opt(&self, name: impl AsRef<str>) -> Option<TestJsonValue<'a>> {
        self.fields.get(name.as_ref()).map(|value| TestJsonValue {
            root: self.root,
            value,
        })
    }

    /// How many keys the object has.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the object has no key.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The object's keys and values.
    pub fn iter(&self) -> impl Iterator<Item = (&'a String, TestJsonValue<'a>)> {
        let root = self.root;
        self.fields
            .iter()
            .map(move |(name, value)| (name, TestJsonValue { root, value }))
    }

    /// The object, as the document holds it.
    fn as_value(&self) -> &'a Value {
        find_object(self.root, self.fields).unwrap_or(self.root)
    }
}

impl fmt::Debug for TestJsonObject<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.fields, f)
    }
}

/// An array of a [`TestJson`] document.
#[derive(Clone, Copy)]
pub struct TestJsonArray<'a> {
    root: &'a Value,
    items: &'a [Value],
}

impl<'a> TestJsonArray<'a> {
    /// The item at `index`.
    ///
    /// # Panics
    ///
    /// When the array is shorter, naming its path and its length.
    #[track_caller]
    pub fn get(&self, index: usize) -> TestJsonValue<'a> {
        self.get_opt(index).unwrap_or_else(|| {
            panic!(
                "`{}` has no index {index}: its length is {}",
                path_to(self.root, self.as_value()),
                self.items.len(),
            )
        })
    }

    /// The item at `index`, when the array has one.
    pub fn get_opt(&self, index: usize) -> Option<TestJsonValue<'a>> {
        self.items.get(index).map(|value| TestJsonValue {
            root: self.root,
            value,
        })
    }

    /// How many items the array has.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the array has no item.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The array's items.
    pub fn iter(&self) -> impl Iterator<Item = TestJsonValue<'a>> {
        let root = self.root;
        self.items
            .iter()
            .map(move |value| TestJsonValue { root, value })
    }

    /// The array, as the document holds it.
    fn as_value(&self) -> &'a Value {
        find_array(self.root, self.items).unwrap_or(self.root)
    }
}

impl fmt::Debug for TestJsonArray<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.items, f)
    }
}

/// The JSON path of `target` in `root` — `$`, `$.data`, `$.items[2]`,
/// `$["a key"]` — found by identity, so a walk pays for it only when an
/// assertion fails.
fn path_to(root: &Value, target: &Value) -> String {
    let mut path = String::from("$");
    if walk(root, target, &mut path) {
        path
    } else {
        String::from("$")
    }
}

/// What kind of JSON value `value` is, as a sentence says it.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn walk(node: &Value, target: &Value, path: &mut String) -> bool {
    if std::ptr::eq(node, target) {
        return true;
    }
    let at = path.len();
    match node {
        Value::Object(fields) => {
            for (name, value) in fields {
                push_key(path, name);
                if walk(value, target, path) {
                    return true;
                }
                path.truncate(at);
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                path.push_str(&format!("[{index}]"));
                if walk(value, target, path) {
                    return true;
                }
                path.truncate(at);
            }
        }
        _ => {}
    }
    false
}

/// `name` as a path step: `.name` when it is an identifier, `["name"]`
/// otherwise.
fn push_key(path: &mut String, name: &str) {
    let plain = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        path.push('.');
        path.push_str(name);
    } else {
        path.push('[');
        path.push_str(&Value::from(name).to_string());
        path.push(']');
    }
}

/// The value in `root` holding exactly `fields`.
fn find_object<'a>(root: &'a Value, fields: &Map<String, Value>) -> Option<&'a Value> {
    find(
        root,
        &|value| matches!(value, Value::Object(map) if std::ptr::eq(map, fields)),
    )
}

/// The value in `root` holding exactly `items`.
fn find_array<'a>(root: &'a Value, items: &[Value]) -> Option<&'a Value> {
    find(
        root,
        &|value| matches!(value, Value::Array(list) if std::ptr::eq(list.as_slice(), items)),
    )
}

fn find<'a>(node: &'a Value, hit: &dyn Fn(&Value) -> bool) -> Option<&'a Value> {
    if hit(node) {
        return Some(node);
    }
    match node {
        Value::Object(fields) => fields.values().find_map(|value| find(value, hit)),
        Value::Array(items) => items.iter().find_map(|value| find(value, hit)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_http::{HeaderName, endpoint_fn};
    use serde_json::json;

    use super::*;
    use crate::TestClient;

    fn document(value: Value) -> TestJson {
        TestJson(value)
    }

    async fn answered(response: fn() -> Response) -> TestResponse {
        TestClient::new(endpoint_fn(move |_req| async move { Ok(response()) }))
            .get("/")
            .send()
            .await
    }

    fn teapot() -> Response {
        Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .header(
                HeaderName::from_static("x-brew"),
                HeaderValue::from_static("earl-grey"),
            )
            .body(r#"{"data":{"items":[{"id":1}],"odd key":true}}"#)
    }

    #[tokio::test]
    #[should_panic(expected = "the response status")]
    async fn a_status_mismatch_names_the_status() {
        answered(teapot).await.assert_status_is_ok();
    }

    #[tokio::test]
    #[should_panic(expected = "the `x-brew` header")]
    async fn a_header_mismatch_names_the_header() {
        answered(teapot).await.assert_header("x-brew", "assam");
    }

    #[tokio::test]
    #[should_panic(expected = "the response has no `x-milk` header")]
    async fn a_missing_header_names_the_header() {
        answered(teapot).await.assert_header("x-milk", "oat");
    }

    #[tokio::test]
    #[should_panic(expected = "the response has a `x-brew` header")]
    async fn a_header_that_should_be_absent_names_the_header() {
        answered(teapot).await.assert_header_is_not_exist("x-brew");
    }

    #[test]
    fn an_invalid_expected_header_value_panics_naming_its_header_and_not_the_value() {
        let resp = TestResponse(Response::builder().body(""));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            resp.assert_header(header::AUTHORIZATION, "Bearer secret\r\n");
        }))
        .expect_err("a malformed expected value panics");
        let message = panic.downcast_ref::<String>().expect("a formatted panic");
        assert!(message.contains("`authorization` header"), "{message}");
        assert!(!message.contains("secret"), "{message}");
    }

    #[tokio::test]
    #[should_panic(expected = "the response body")]
    async fn a_body_mismatch_names_the_body() {
        answered(teapot).await.assert_text("{}").await;
    }

    #[tokio::test]
    async fn the_header_and_body_assertions_pass_on_what_was_answered() {
        let resp = answered(teapot).await;
        resp.assert_status(StatusCode::IM_A_TEAPOT);
        resp.assert_header("x-brew", "earl-grey");
        resp.assert_header_all("x-brew", ["earl-grey"]);
        resp.assert_header_exist("x-brew");
        resp.assert_header_is_not_exist("x-milk");
        resp.assert_json(json!({ "data": { "items": [{ "id": 1 }], "odd key": true } }))
            .await;
    }

    #[test]
    #[should_panic(expected = "`$.data` has no key `count`")]
    fn a_missing_key_names_its_object_path() {
        let json = document(json!({ "data": { "items": [] } }));
        let _ = json.value().object().get("data").object().get("count");
    }

    #[test]
    #[should_panic(expected = "`$.data.items` has no index 3: its length is 1")]
    fn a_missing_index_names_its_array_path_and_length() {
        let json = document(json!({ "data": { "items": [1] } }));
        let _ = json
            .value()
            .object()
            .get("data")
            .object()
            .get("items")
            .array()
            .get(3);
    }

    #[test]
    #[should_panic(expected = "`$.data.items[0].id` is a number, not a string")]
    fn a_kind_mismatch_names_the_value_path() {
        let json = document(json!({ "data": { "items": [{ "id": 1 }] } }));
        let _ = json
            .value()
            .object()
            .get("data")
            .object()
            .get("items")
            .array()
            .get(0)
            .object()
            .get("id")
            .string();
    }

    #[test]
    #[should_panic(expected = "`$[\"odd key\"]`")]
    fn a_key_that_is_no_identifier_is_quoted_in_the_path() {
        let json = document(json!({ "odd key": "yes" }));
        json.value().object().get("odd key").assert_string("no");
    }

    #[test]
    fn the_document_serializes_as_it_was_read() {
        let read = json!({ "data": { "items": [1, 2] } });
        assert_eq!(serde_json::to_value(document(read.clone())).unwrap(), read);
    }

    #[test]
    fn the_walk_reads_what_the_document_holds() {
        let json = document(json!({ "items": [{ "id": 7, "title": "Hello" }], "total": 1 }));
        let root = json.value().object();
        let items = root.get("items").array();
        assert_eq!(items.len(), 1);
        assert!(!items.is_empty());
        assert!(root.get_opt("missing").is_none());
        assert_eq!(root.get("total").i64(), 1);
        let first = items.iter().next().unwrap().object();
        assert_eq!(first.get("id").deserialize::<u32>(), 7);
        first.get("title").assert_string("Hello");
        assert_eq!(
            root.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["items", "total"]
        );
    }
}
