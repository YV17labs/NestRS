//! Typed request-header extractor — the header-map twin of poem's `Query<T>`.
//!
//! [`Header<T>`] deserializes `T` from the request's headers: one struct field
//! per header, `#[serde(rename = "…")]` spelling the wire name, and an
//! `Option<_>` field marking the header optional. Lookup is case-insensitive,
//! as HTTP header names are — the typed path asks [`HeaderMap`], which is.
//!
//! **`#[serde(flatten)]` is the one shape that rule does not reach, and it is a
//! serde constraint rather than a choice here.** A struct carrying a flattened
//! field is deserialized through [`FromHeaders::deserialize_map`] instead of
//! `deserialize_struct` — serde never hands over the field list — so the keys
//! are the names [`HeaderMap`] *stores*, which are lowercased, and serde then
//! matches a flattened field's `rename` against them **case-sensitively**. A
//! flattened field must therefore spell its header in lowercase
//! (`#[serde(rename = "x-request-id")]`); spelled `X-Request-Id` it binds `None`
//! on every request. Pinned by
//! `header::a_flattened_field_matches_its_header_in_lowercase`.
//!
//! **A field naming something that is not a header name is refused**, rather
//! than binding `None` forever: `http` implements its lookup by failing, so
//! `X-Tenant:` or `X Request Id` would be indistinguishable from a header the
//! caller never sent.
//!
//! A missing required header, or a value that does not parse into the field's
//! type, is rejected at the edge with the same RFC-9457
//! `application/problem+json` `400` a pipe rejection carries.
//!
//! **The rejection names the header and never quotes its value.** A header is
//! where credentials travel (`Authorization`, `Cookie`, an API key), and a `400`
//! body is logged, cached and proxied — the same reason
//! [`Valid`](crate::Valid)'s rejection reports the failing field without
//! echoing what was submitted. serde's own constructors do quote it
//! (`unknown variant \`…\``), so [`HeaderError`] overrides every one of them
//! that does.
//!
//! **A header sent twice binds its first value**, on both the typed and the
//! untyped path — see [`FromHeaders::deserialize_map`].
//!
//! `#[routes]` captures each `Header<T>` payload into
//! [`HttpRouteMeta::header_params`](crate::HttpRouteMeta::header_params), so
//! the OpenAPI document carries one `in: header` parameter per property of `T`.

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use poem::http::HeaderMap;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use serde::de::value::MapDeserializer;
use serde::de::{DeserializeOwned, Deserializer, IntoDeserializer, Visitor};
use serde::forward_to_deserialize_any;

use crate::ProblemDetails;

/// Request headers deserialized into `T`.
///
/// ```ignore
/// #[input]
/// struct Tracing {
///     #[serde(rename = "X-Request-Id")]
///     request_id: Option<String>,
/// }
///
/// #[get("/things")]
/// async fn list(&self, tracing: Header<Tracing>) -> Json<Vec<Thing>> { /* … */ }
/// ```
pub struct Header<T>(pub T);

impl<T> Header<T> {
    /// Take ownership of the deserialized headers.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Header<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'a, T: DeserializeOwned> FromRequest<'a> for Header<T> {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        T::deserialize(FromHeaders {
            headers: req.headers(),
        })
        .map(Header)
        .map_err(reject)
    }
}

/// One error format at the edge: the `400` RFC-9457 `application/problem+json`
/// [`ProblemDetails`] every other edge rejection renders, with the header
/// diagnostic as `detail`.
fn reject(err: HeaderError) -> Error {
    Error::from(ProblemDetails::bad_request().with_detail(err.to_string()))
}

/// What a header binding can fail on. Each variant names the header; none
/// carries its value.
#[derive(Debug)]
enum HeaderError {
    /// A field without a default (i.e. not `Option<_>`) whose header is absent.
    Missing(String),
    /// The header is present but its value is not what the field's type needs.
    Malformed {
        name: String,
        expected: Cow<'static, str>,
    },
    /// serde recognised the shape and refused the content — an enum field whose
    /// text names no variant, a `Deserialize` impl calling `invalid_value`. It
    /// carries what was *expected*, never what was read; the header's name is
    /// put back by [`against`](Self::against), which is the one place that
    /// knows it.
    Unexpected(Cow<'static, str>),
    /// Anything serde itself reports — a `deserialize_with` function, a custom
    /// `Deserialize` impl.
    Custom(String),
    /// A field naming something that cannot be a header name. The developer's
    /// mistake, not the caller's — but it surfaces on a request, so it is
    /// reported the same way and says whose it is.
    NotAHeaderName(String),
}

impl HeaderError {
    fn not_a_header_name(field: &str) -> Self {
        Self::NotAHeaderName(field.to_owned())
    }

    fn malformed(name: &str, expected: impl Into<Cow<'static, str>>) -> Self {
        Self::Malformed {
            name: name.to_owned(),
            expected: expected.into(),
        }
    }

    /// Attribute a content refusal to the header it was read from.
    ///
    /// serde builds `unknown_variant` and friends from **static**
    /// constructors — there is no deserializer in scope to ask which header is
    /// being read — so the name is attached here, by the arm that has one.
    /// Every other variant already names a header, which makes this idempotent
    /// and safe to apply at each arm that hands a value to a visitor.
    fn against(self, name: &str) -> Self {
        match self {
            Self::Unexpected(expected) => Self::malformed(name, expected),
            named => named,
        }
    }
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "missing required header `{name}`"),
            Self::Malformed { name, expected } => {
                write!(f, "header `{name}` is not {expected}")
            }
            Self::Unexpected(expected) => write!(f, "header value is not {expected}"),
            Self::Custom(msg) => f.write_str(msg),
            Self::NotAHeaderName(field) => write!(
                f,
                "`{field}` is not a valid header name, so no request can carry it — fix the \
                 field's `#[serde(rename = \"…\")]`",
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

/// serde's `expected one of \`a\`, \`b\`` list, without the value its own
/// message opens with.
fn one_of(expected: &'static [&'static str]) -> String {
    let list: Vec<String> = expected.iter().map(|item| format!("`{item}`")).collect();
    format!("one of {}", list.join(", "))
}

impl serde::de::Error for HeaderError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Custom(msg.to_string())
    }

    /// serde's derive routes an absent field here, which is what turns
    /// "missing field `X-Request-Id`" into a sentence about headers.
    fn missing_field(field: &'static str) -> Self {
        Self::Missing(field.to_owned())
    }

    /// The one serde constructor a plain header field actually reaches: an
    /// enum-typed field whose text names no variant. The default
    /// (`unknown variant \`{variant}\`, expected …`) opens with the value read
    /// off the wire, which on a header is exactly what must not be echoed.
    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// Same default shape (`unknown field \`{field}\``); a header *name* is not
    /// a secret, but one rule over every value-interpolating constructor is
    /// what stops the next one being missed.
    fn unknown_field(_field: &str, expected: &'static [&'static str]) -> Self {
        Self::Unexpected(one_of(expected).into())
    }

    /// `invalid value: string "…", expected …` — one custom `Deserialize` impl
    /// away, and it quotes the value in full.
    fn invalid_value(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }

    /// `invalid type: string "…", expected …`, same reasoning.
    fn invalid_type(
        _unexpected: serde::de::Unexpected<'_>,
        expected: &dyn serde::de::Expected,
    ) -> Self {
        Self::Unexpected(expected.to_string().into())
    }
}

/// Deserializer over a request's [`HeaderMap`].
///
/// `deserialize_struct` receives the field names serde derived, so the map is
/// built by asking the header map for *those* names — the lookup `http`
/// performs case-insensitively — rather than by iterating stored (lowercased)
/// names and hoping they match how the developer spelled the rename.
struct FromHeaders<'a> {
    headers: &'a HeaderMap,
}

impl<'de> Deserializer<'de> for FromHeaders<'_> {
    type Error = HeaderError;

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, HeaderError> {
        let mut pairs: Vec<(String, Part)> = Vec::with_capacity(fields.len());
        for field in fields {
            let Some(raw) = self.headers.get(*field) else {
                // Absent — or not a header name at all. `http` implements
                // `AsHeaderName for &str` by *failing the lookup*, so the two
                // are one answer here: an `Option<_>` field would bind `None` on
                // every request forever, and a required one would 400 telling
                // the caller to send a header no client can send. Neither points
                // at the mistake, which is in the `rename`.
                //
                // Asked only on this branch, and that is the whole cost: a
                // successful lookup has already proved the name is valid, since
                // `HeaderMap::get` parses it to answer at all.
                if poem::http::HeaderName::from_bytes(field.as_bytes()).is_err() {
                    return Err(HeaderError::not_a_header_name(field));
                }
                continue;
            };
            let value = raw
                .to_str()
                .map_err(|_| HeaderError::malformed(field, "valid UTF-8"))?;
            pairs.push(((*field).to_owned(), Part::new(field, value)));
        }
        visitor.visit_map(MapDeserializer::new(pairs.into_iter()))
    }

    /// The untyped form (`HashMap<String, String>`). A header whose value is
    /// not UTF-8 is skipped rather than failing the whole map: no field asked
    /// for it, so there is nothing to report against.
    ///
    /// Iterates **names** rather than entries, so a header sent twice binds its
    /// first value — what [`HeaderMap::get`] gives the typed path above. Over
    /// entries the last one would have won instead, and one extractor answering
    /// "which value wins" two ways is a difference nobody would look for.
    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, HeaderError> {
        let pairs: Vec<(String, Part)> = self
            .headers
            .keys()
            .filter_map(|name| {
                let value = self.headers.get(name)?.to_str().ok()?;
                Some((name.as_str().to_owned(), Part::new(name.as_str(), value)))
            })
            .collect();
        visitor.visit_map(MapDeserializer::new(pairs.into_iter()))
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, HeaderError> {
        self.deserialize_map(visitor)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct enum identifier ignored_any
    }
}

/// One header's value, deserialized into whatever the field's type is.
///
/// A header value is a string on the wire, so the numeric and boolean arms
/// parse it — the same coercion `Query<T>` gets from its form decoder, without
/// which a `u32` field could only ever be a `String`.
struct Part {
    name: String,
    value: String,
}

impl Part {
    fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_owned(),
            // Header values arrive without their optional surrounding
            // whitespace, but a client that sent `id: 12 ` should not be told
            // its integer is malformed.
            value: value.trim().to_owned(),
        }
    }
}

/// The scalar arms, each parsing the header's text into the field's type and
/// reporting the *kind* it expected — never the value it read.
macro_rules! parse_arms {
    ($($method:ident => $visit:ident, $ty:ty, $expected:literal;)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, HeaderError> {
                match self.value.parse::<$ty>() {
                    Ok(value) => visitor.$visit(value),
                    Err(_) => Err(HeaderError::malformed(&self.name, $expected)),
                }
            }
        )*
    };
}

impl<'de> Deserializer<'de> for Part {
    type Error = HeaderError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, HeaderError> {
        let name = self.name;
        visitor
            .visit_string(self.value)
            .map_err(|err: HeaderError| err.against(&name))
    }

    parse_arms! {
        deserialize_bool => visit_bool, bool, "a boolean";
        deserialize_i8 => visit_i8, i8, "an integer";
        deserialize_i16 => visit_i16, i16, "an integer";
        deserialize_i32 => visit_i32, i32, "an integer";
        deserialize_i64 => visit_i64, i64, "an integer";
        deserialize_i128 => visit_i128, i128, "an integer";
        deserialize_u8 => visit_u8, u8, "an integer";
        deserialize_u16 => visit_u16, u16, "an integer";
        deserialize_u32 => visit_u32, u32, "an integer";
        deserialize_u64 => visit_u64, u64, "an integer";
        deserialize_u128 => visit_u128, u128, "an integer";
        deserialize_f32 => visit_f32, f32, "a number";
        deserialize_f64 => visit_f64, f64, "a number";
        deserialize_char => visit_char, char, "a single character";
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, HeaderError> {
        // Present, so `Some` — an absent header never reaches here (serde's
        // derive fills the field through `missing_field`, which answers `None`).
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, HeaderError> {
        visitor.visit_newtype_struct(self)
    }

    /// An enum-typed field: the header's text names a unit variant.
    ///
    /// The refusal serde raises for a text that names none is
    /// [`unknown_variant`](serde::de::Error::unknown_variant), a static
    /// constructor that cannot know which header it was reading — so the name
    /// goes back on here, and the value serde would have quoted never existed
    /// in the message.
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, HeaderError> {
        let name = self.name;
        visitor
            .visit_enum(self.value.into_deserializer())
            .map_err(|err: HeaderError| err.against(&name))
    }

    // A header carries one scalar. Rejecting the compound shapes here rather
    // than forwarding them to `deserialize_any` keeps the diagnostic ours:
    // serde's own type-mismatch message quotes the unexpected value, and that
    // value is a header's.
    fn deserialize_seq<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, HeaderError> {
        Err(HeaderError::malformed(&self.name, "a list"))
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, HeaderError> {
        Err(HeaderError::malformed(&self.name, "a list"))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, HeaderError> {
        Err(HeaderError::malformed(&self.name, "a list"))
    }

    fn deserialize_map<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, HeaderError> {
        Err(HeaderError::malformed(&self.name, "an object"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, HeaderError> {
        Err(HeaderError::malformed(&self.name, "an object"))
    }

    forward_to_deserialize_any! {
        str string bytes byte_buf unit unit_struct identifier ignored_any
    }
}

impl<'de> IntoDeserializer<'de, HeaderError> for Part {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use poem::http::{HeaderValue, StatusCode};
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct Tracing {
        #[serde(rename = "X-Request-Id")]
        request_id: String,
        #[serde(rename = "X-Retry-Count")]
        retry: Option<u32>,
        #[serde(rename = "X-Debug")]
        debug: Option<bool>,
    }

    async fn extract<T: DeserializeOwned>(headers: &[(&str, HeaderValue)]) -> Result<Header<T>> {
        let mut builder = Request::builder();
        for (name, value) in headers {
            builder = builder.header(*name, value.clone());
        }
        let (req, mut body) = builder.finish().split();
        Header::<T>::from_request(&req, &mut body).await
    }

    fn value(v: &str) -> HeaderValue {
        HeaderValue::from_str(v).expect("a header value")
    }

    async fn detail_of(err: Error) -> String {
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.headers()
                .get(poem::http::header::CONTENT_TYPE)
                .map(|v| v.as_bytes()),
            Some(b"application/problem+json".as_slice()),
            "a header rejection renders as RFC-9457, like every other edge error",
        );
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["status"], 400);
        json["detail"].as_str().unwrap_or_default().to_owned()
    }

    #[tokio::test]
    async fn binds_a_renamed_header_case_insensitively() {
        // The wire spelling is `X-Request-Id`; HTTP/2 (and `http`'s own store)
        // lowercases it. Both must reach the same field.
        let h: Header<Tracing> = extract(&[("x-request-id", value("abc-123"))])
            .await
            .expect("the header binds");
        assert_eq!(h.request_id, "abc-123");
        assert_eq!(h.retry, None, "an absent optional header is None");
        assert_eq!(h.debug, None);
    }

    #[tokio::test]
    async fn parses_a_typed_header_out_of_its_text() {
        let h: Header<Tracing> = extract(&[
            ("X-Request-Id", value("abc")),
            ("X-Retry-Count", value("7")),
            ("X-Debug", value("true")),
        ])
        .await
        .expect("the typed headers bind");
        assert_eq!(h.retry, Some(7));
        assert_eq!(h.debug, Some(true));
    }

    #[tokio::test]
    async fn a_missing_required_header_is_a_400_naming_it() {
        let err = extract::<Tracing>(&[("X-Retry-Count", value("1"))])
            .await
            .err()
            .expect("the required header is absent");
        assert_eq!(
            detail_of(err).await,
            "missing required header `X-Request-Id`",
        );
    }

    #[tokio::test]
    async fn a_value_that_does_not_parse_is_a_400_naming_the_header() {
        let err = extract::<Tracing>(&[
            ("X-Request-Id", value("abc")),
            ("X-Retry-Count", value("soon")),
        ])
        .await
        .err()
        .expect("`soon` is not a u32");
        assert_eq!(
            detail_of(err).await,
            "header `X-Retry-Count` is not an integer",
        );
    }

    /// The pendant of `valid_rejection_does_not_echo_the_submitted_value_…`:
    /// headers are where credentials travel, so a rejection that quoted the
    /// value would leak one into every log and proxy cache that keeps a `400`.
    #[tokio::test]
    async fn a_rejection_never_echoes_the_header_value() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code, reason = "the binding is what is under test, and it fails")]
        struct Auth {
            #[serde(rename = "X-Api-Key")]
            key: u64,
        }
        let err = extract::<Auth>(&[("X-Api-Key", value("sk-live-super-secret"))])
            .await
            .err()
            .expect("the key is not a u64");
        let detail = detail_of(err).await;
        assert!(
            !detail.contains("super-secret"),
            "the value must not reach the response body: {detail}",
        );
        assert!(detail.contains("X-Api-Key"), "{detail}");

        // The enum arm, which the scalar case above never exercises: serde's
        // own `unknown_variant` opens with the text it read, so a field typed
        // as an enum was the one shape that echoed a header into the `400`.
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Mode {
            Fast,
            Slow,
        }
        #[derive(Debug, Deserialize)]
        #[allow(dead_code, reason = "the binding is what is under test, and it fails")]
        struct Prefs {
            #[serde(rename = "X-Mode")]
            mode: Mode,
        }
        let err = extract::<Prefs>(&[("X-Mode", value("sk-live-super-secret"))])
            .await
            .err()
            .expect("no variant is spelled that");
        let detail = detail_of(err).await;
        assert!(
            !detail.contains("super-secret"),
            "an enum field must not echo it either: {detail}",
        );
        assert_eq!(
            detail, "header `X-Mode` is not one of `fast`, `slow`",
            "and what a caller needs is the variants it may send",
        );
    }

    #[tokio::test]
    async fn a_non_utf8_value_is_reported_against_its_header() {
        #[derive(Debug, Deserialize)]
        #[allow(dead_code, reason = "the binding is what is under test, and it fails")]
        struct Opaque {
            #[serde(rename = "X-Blob")]
            blob: String,
        }
        let err = extract::<Opaque>(&[(
            "X-Blob",
            HeaderValue::from_bytes(&[0xff, 0xfe]).expect("an opaque header value"),
        )])
        .await
        .err()
        .expect("the value is not UTF-8");
        assert_eq!(detail_of(err).await, "header `X-Blob` is not valid UTF-8");
    }

    #[tokio::test]
    async fn an_untyped_map_binds_every_header() {
        let h: Header<HashMap<String, String>> =
            extract(&[("X-One", value("1")), ("X-Two", value("2"))])
                .await
                .expect("the map binds");
        assert_eq!(h.get("x-one").map(String::as_str), Some("1"));
        assert_eq!(h.get("x-two").map(String::as_str), Some("2"));
    }

    #[tokio::test]
    async fn an_enum_header_binds_its_unit_variant() {
        #[derive(Debug, Deserialize, PartialEq)]
        #[serde(rename_all = "lowercase")]
        enum Mode {
            Fast,
            Slow,
        }
        #[derive(Debug, Deserialize)]
        struct Prefs {
            #[serde(rename = "X-Mode")]
            mode: Mode,
        }
        let h: Header<Prefs> = extract(&[("X-Mode", value("fast"))])
            .await
            .expect("the enum binds");
        assert_eq!(h.mode, Mode::Fast);
    }

    #[test]
    fn into_inner_and_deref_expose_the_payload() {
        let h = Header("value".to_string());
        assert_eq!(h.len(), 5);
        assert_eq!(h.into_inner(), "value");
    }

    #[test]
    fn a_surrounding_space_does_not_make_a_number_malformed() {
        let part = Part::new("X-Retry-Count", " 7 ");
        assert_eq!(part.value, "7");
    }
}
