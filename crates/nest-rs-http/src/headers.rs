//! Typed request-header extractor — the header-map twin of poem's `Query<T>`.
//!
//! [`Header<T>`] deserializes `T` from the request's headers: one struct field
//! per header, `#[serde(rename = "…")]` spelling the wire name, and an
//! `Option<_>` field marking the header optional. Lookup is case-insensitive;
//! a header sent twice binds its first value.
//!
//! **A `#[serde(flatten)]` field must spell its header in lowercase**: serde then
//! deserializes through `deserialize_map`, and matches the stored (lowercased)
//! names case-sensitively — `X-Request-Id` would bind `None` on every request.
//! A `rename` that is not a valid header name is refused rather than binding
//! `None` forever.
//!
//! A missing required header, or a value that does not parse into the field's
//! type, is an RFC-9457 `400` that names the header and **never quotes its
//! value**: a header is where credentials travel.

use std::ops::Deref;

use poem::http::HeaderMap;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use serde::de::{
    DeserializeOwned, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, Visitor,
};
use serde::forward_to_deserialize_any;

use crate::ProblemDetails;

use crate::error::HeaderError;

/// Request headers deserialized into `T`.
///
/// ```
/// # use nest_rs_core::module;
/// # use nest_rs_http::poem::web::Json;
/// # use nest_rs_http::{Header, controller, input, routes};
/// # use nest_rs_testing::TestApp;
/// #[input]
/// struct Tracing {
///     #[serde(rename = "X-Request-Id")]
///     request_id: Option<String>,
/// }
/// # #[input]
/// # struct Thing {
/// #     request_id: Option<String>,
/// # }
/// # #[controller(path = "/")]
/// # #[derive(Default)]
/// # struct ThingsController;
/// # #[routes]
/// # impl ThingsController {
///
/// #[get("/things")]
/// async fn list(&self, tracing: Header<Tracing>) -> Json<Vec<Thing>> {
///     Json(vec![Thing { request_id: tracing.into_inner().request_id }])
/// }
/// # }
/// # #[module(providers = [ThingsController])]
/// # struct ThingsModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = TestApp::for_module::<ThingsModule>().await?;
///
/// let reply = app.http().get("/things").header("x-request-id", "abc-123").send().await;
/// reply.assert_json(serde_json::json!([{ "request_id": "abc-123" }])).await;
/// # Ok(())
/// # }
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

fn reject(err: HeaderError) -> Error {
    Error::from(ProblemDetails::bad_request().with_detail(err.to_string()))
}

/// serde's `expected one of \`a\`, \`b\`` list, without the value its own
/// message opens with.
pub(crate) fn one_of(expected: &'static [&'static str]) -> String {
    let list: Vec<String> = expected.iter().map(|item| format!("`{item}`")).collect();
    format!("one of {}", list.join(", "))
}

/// Deserializer over a request's [`HeaderMap`]; `deserialize_struct` asks it for
/// serde's field names, which `http` looks up case-insensitively.
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
        let mut parts: Vec<Part> = Vec::with_capacity(fields.len());
        for field in fields {
            let Some(raw) = self.headers.get(*field) else {
                // `http` answers an invalid name as absent, so a bad `rename` would
                // bind `None` forever; a hit has already proved its name valid.
                if poem::http::HeaderName::from_bytes(field.as_bytes()).is_err() {
                    return Err(HeaderError::not_a_header_name(field));
                }
                continue;
            };
            #[expect(
                clippy::map_err_ignore,
                reason = "ToStrError says no more than the refusal, which names the header"
            )]
            let value = raw
                .to_str()
                .map_err(|_| HeaderError::malformed(field, "valid UTF-8"))?;
            parts.push(Part::new(field, value));
        }
        visitor.visit_map(Headers::new(parts))
    }

    /// The untyped form (`HashMap<String, String>`); a non-UTF-8 value is skipped.
    ///
    /// Iterates **names**, not entries, so a header sent twice binds its first
    /// value as the typed path does; over entries the last one would win.
    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, HeaderError> {
        let parts: Vec<Part> = self
            .headers
            .keys()
            .filter_map(|name| {
                let value = self.headers.get(name)?.to_str().ok()?;
                Some(Part::new(name.as_str(), value))
            })
            .collect();
        visitor.visit_map(Headers::new(parts))
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

/// The headers a visitor asked for, each value's refusal named against its header.
///
/// Not serde's `MapDeserializer`: a type's `custom` refusal arrives after the
/// header's deserializer returned, so only the map knows whose value it was.
struct Headers {
    parts: std::vec::IntoIter<Part>,
    value: Option<Part>,
}

impl Headers {
    fn new(parts: Vec<Part>) -> Self {
        Self {
            parts: parts.into_iter(),
            value: None,
        }
    }
}

impl<'de> MapAccess<'de> for Headers {
    type Error = HeaderError;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, HeaderError> {
        let Some(part) = self.parts.next() else {
            return Ok(None);
        };
        let key = seed.deserialize(IntoDeserializer::<HeaderError>::into_deserializer(
            part.name.as_str(),
        ))?;
        self.value = Some(part);
        Ok(Some(key))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, HeaderError> {
        // serde's contract asks for a value only after its key.
        let Some(part) = self.value.take() else {
            return Err(HeaderError::Refused);
        };
        let name = part.name.clone();
        seed.deserialize(part).map_err(|err| err.against(&name))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.parts.len())
    }
}

/// One header's value, deserialized into the field's type; the scalar arms parse
/// its text, the coercion `Query<T>` gets from its form decoder.
struct Part {
    name: String,
    value: String,
}

impl Part {
    fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_owned(),
            // A client that sent `id: 12 ` must not be told its integer is malformed.
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
        // An absent header never reaches here: serde's derive answers `None`
        // through `missing_field`.
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
    /// serde's [`unknown_variant`](serde::de::Error::unknown_variant) cannot know
    /// which header it read, so the name goes back on here.
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

    // Not forwarded to `deserialize_any`: serde's type-mismatch message quotes
    // the value, and that value is a header's.
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
    async fn a_type_s_own_refusal_names_the_header_and_never_quotes_it() {
        #[derive(Debug)]
        struct ApiKey;
        impl<'de> Deserialize<'de> for ApiKey {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                Err(serde::de::Error::custom(format!("bad key {raw}")))
            }
        }
        #[derive(Debug, Deserialize)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts the error, never a value"
        )]
        struct Keyed {
            #[serde(rename = "X-Api-Key")]
            key: ApiKey,
        }
        let err = extract::<Keyed>(&[("X-Api-Key", value("sk_live_51HsecretTOKEN"))])
            .await
            .err()
            .expect("the type refuses it");
        assert_eq!(
            detail_of(err).await,
            "header `X-Api-Key` is not a value its type accepts",
        );
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

    #[tokio::test]
    async fn a_rejection_never_echoes_the_header_value() {
        #[derive(Debug, Deserialize)]
        #[expect(dead_code, reason = "the binding is what is under test, and it fails")]
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

        // The enum arm: serde's own `unknown_variant` opens with the text it read.
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Mode {
            Fast,
            Slow,
        }
        #[derive(Debug, Deserialize)]
        #[expect(dead_code, reason = "the binding is what is under test, and it fails")]
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
        #[expect(dead_code, reason = "the binding is what is under test, and it fails")]
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
