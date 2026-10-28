//! The token endpoint's exchange — RFC 6749 §4.4.2's request, read as §3.2
//! and §5.2 say, and §5.1's response.

use std::fmt;

use nest_rs_core::Secret;
use nest_rs_http::__private::{DocumentedBody, schema_of};
use nest_rs_http::{
    Form, FromRequest, HeaderValue, HttpError, IntoResponse, Json, Request, RequestBody,
    RequestBodyMeta, Response, StatusCode, header,
};
use schemars::JsonSchema;
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::MalformedRequest;

/// RFC 6749 §4.4.2 — the `client_credentials` access token request.
///
/// `grant_type` is a `String`: §5.2 answers an unknown grant with
/// [`TokenError::UnsupportedGrant`](crate::TokenError::UnsupportedGrant), which an
/// enum would turn into a deserialization error the issuer never sees.
///
/// A handler takes it as a parameter, which reads the request's
/// `application/x-www-form-urlencoded` body (§4.4.2) and documents it so; a
/// request that does not parse is refused with §5.2's `invalid_request`, its
/// `error_description` saying where — a parameter missing, sent with no value
/// (§3.2: "treated as if they were omitted"), repeated or of the wrong kind, a
/// body that is no form, a client credential in the body beside the
/// `Authorization` header (more than one mechanism) — never what it held. An
/// empty `scope` or `resource` is none, and a parameter the request does not
/// define is ignored (§3.2). `resource` alone may repeat (RFC 8707 §2). A body
/// past the transport's cap keeps the transport's `413`.
///
/// That parameter is its one reader: the type derives no `Deserialize`, so no
/// other spelling — a `Form<Self>`, a `Json<Self>` — skips the refusals above.
///
/// ```compile_fail,E0277
/// use nest_rs_http::{Form, FromRequest};
/// use nest_rs_oauth_server::AccessTokenRequest;
///
/// fn extractor<T: for<'a> FromRequest<'a>>() {}
///
/// extractor::<Form<AccessTokenRequest>>();
/// ```
#[derive(Debug, JsonSchema)]
// schemars would otherwise publish this whole rustdoc as the schema's description.
#[schemars(description = "RFC 6749 §4.4.2 — the `client_credentials` access token request.")]
#[non_exhaustive]
pub struct AccessTokenRequest {
    /// The grant being exercised — `client_credentials` at this endpoint.
    pub grant_type: String,
    /// §3.3 space-delimited scope list. Absent ⇒ the issuer's own default.
    #[schemars(default)]
    pub scope: Option<String>,
    /// RFC 8707 §2's `resource` parameters, in the order sent: the resources
    /// the token is requested for, which become its audience. Empty ⇒ the
    /// issuer's own default;
    /// [`AuthenticatedClient::audience_for`](crate::AuthenticatedClient::audience_for)
    /// chooses among them.
    #[schemars(default)]
    pub resource: Vec<String>,
}

/// §4.4.2's parameters as the body spells them, with the client credentials
/// §2.3.1 and RFC 7521 §4.2 place there, read for their presence alone.
///
/// Read by hand: `resource` may repeat (RFC 8707 §2), which a derived reader
/// refuses as a duplicate, and every other parameter may not (§3.2).
struct Parameters {
    grant_type: String,
    scope: Option<String>,
    resource: Vec<String>,
    client_secret: Credential,
    client_assertion: Credential,
    client_assertion_type: Credential,
}

impl<'de> Deserialize<'de> for Parameters {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // As a struct, so the edge places a missing or repeated one at its name.
        const FIELDS: &[&str] = &[
            "grant_type",
            "scope",
            "resource",
            "client_secret",
            "client_assertion",
            "client_assertion_type",
        ];
        deserializer.deserialize_struct("Parameters", FIELDS, ParametersVisitor)
    }
}

/// Reads [`Parameters`] off the form's pairs, refusing a repeat as serde's
/// derive words it, so the edge says where the request failed alike.
struct ParametersVisitor;

impl<'de> Visitor<'de> for ParametersVisitor {
    type Value = Parameters;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the token request's form parameters")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Parameters, A::Error> {
        /// The value of a parameter §3.2 sends at most once.
        fn once<'de, A: MapAccess<'de>, T: Deserialize<'de>>(
            map: &mut A,
            slot: &mut Option<T>,
            name: &'static str,
        ) -> Result<(), A::Error> {
            if slot.is_some() {
                return Err(de::Error::duplicate_field(name));
            }
            *slot = Some(map.next_value()?);
            Ok(())
        }

        let mut grant_type = None;
        let mut scope = None;
        let mut resource = Vec::new();
        let mut client_secret = None;
        let mut client_assertion = None;
        let mut client_assertion_type = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "grant_type" => once(&mut map, &mut grant_type, "grant_type")?,
                "scope" => once(&mut map, &mut scope, "scope")?,
                "resource" => resource.push(map.next_value::<String>()?),
                "client_secret" => once(&mut map, &mut client_secret, "client_secret")?,
                "client_assertion" => once(&mut map, &mut client_assertion, "client_assertion")?,
                "client_assertion_type" => {
                    once(
                        &mut map,
                        &mut client_assertion_type,
                        "client_assertion_type",
                    )?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Parameters {
            grant_type: grant_type.ok_or_else(|| de::Error::missing_field("grant_type"))?,
            scope,
            resource,
            client_secret: client_secret.unwrap_or_default(),
            client_assertion: client_assertion.unwrap_or_default(),
            client_assertion_type: client_assertion_type.unwrap_or_default(),
        })
    }
}

/// Whether a body parameter carrying a client credential has a value (§3.2);
/// the value itself is never kept.
#[derive(Default)]
struct Credential(bool);

impl<'de> Deserialize<'de> for Credential {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|value| Self(!value.is_empty()))
    }
}

/// What a client that authenticates twice is told: §5.2's "utilizes more
/// than one mechanism for authenticating the client".
const TWO_MECHANISMS: &str =
    "the client authenticates by more than one mechanism: the `Authorization` header and the body";

/// What `grant_type` sent with no value is told: what a request omitting it
/// is (§3.2).
const NO_GRANT_TYPE: &str = "form field `grant_type`: missing";

impl<'a> FromRequest<'a> for AccessTokenRequest {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> nest_rs_http::Result<Self> {
        let Form(sent) = Form::<Parameters>::from_request(req, body)
            .await
            .map_err(malformed)?;
        if sent.grant_type.is_empty() {
            return Err(MalformedRequest::at(NO_GRANT_TYPE).into());
        }
        let in_body =
            sent.client_secret.0 || sent.client_assertion.0 || sent.client_assertion_type.0;
        if in_body && req.headers().contains_key(header::AUTHORIZATION) {
            return Err(MalformedRequest::at(TWO_MECHANISMS).into());
        }
        Ok(Self {
            grant_type: sent.grant_type,
            scope: sent.scope.filter(|scope| !scope.is_empty()),
            resource: sent
                .resource
                .into_iter()
                .filter(|resource| !resource.is_empty())
                .collect(),
        })
    }
}

/// The form's refusal as §5.2's: a body that is no form (`415`) or a form
/// that does not decode (`400`) is `invalid_request`, said as the edge says
/// it; anything else — a body past the cap — keeps the transport's answer.
fn malformed(refused: HttpError) -> HttpError {
    match refused.status() {
        StatusCode::BAD_REQUEST | StatusCode::UNSUPPORTED_MEDIA_TYPE => {
            MalformedRequest::at(&refused.to_string()).into()
        }
        _ => refused,
    }
}

/// The body a route reading it documents: §4.4.2's form.
impl DocumentedBody for AccessTokenRequest {
    fn request_body() -> RequestBodyMeta {
        RequestBodyMeta::Form(schema_of::<Self>)
    }
}

/// RFC 6749 §5.1 — the successful access token response.
///
/// A handler returns it bare: it renders as its JSON with §5.1's
/// `Cache-Control: no-store` and `Pragma: no-cache`. It is no `Serialize`, so
/// no other spelling of the response — a `Json<Self>`, a hand-built body — can
/// drop the two. `#[api(response = …)]` names it for the OpenAPI document,
/// which reads a body's type off a `Json<T>` return. `Debug` redacts the token.
///
/// An issuer that grants another scope than the one requested — a narrower
/// set, or its default when the request named none — states it with
/// [`with_scope`](Self::with_scope), as §3.3 requires.
///
/// ```
/// # use nest_rs_http::{controller, routes};
/// use nest_rs_oauth_server::AccessTokenResponse;
/// # fn sign_in() -> AccessTokenResponse {
/// #     AccessTokenResponse::bearer("t", 60).with_scope(["posts:read"])
/// # }
/// # #[controller(path = "/")]
/// # struct LoginController;
///
/// #[routes]
/// impl LoginController {
///     #[post("/login")]
///     #[public]
///     #[api(response = AccessTokenResponse)]
///     async fn login(&self) -> nest_rs_http::Result<AccessTokenResponse> {
///         Ok(sign_in())
///     }
/// }
/// ```
///
/// The token never leaves without them:
///
/// ```compile_fail,E0277
/// use nest_rs_http::{IntoResponse, Json};
/// use nest_rs_oauth_server::AccessTokenResponse;
///
/// let cacheable = Json(AccessTokenResponse::bearer("t", 60)).into_response();
/// ```
#[derive(Debug, JsonSchema)]
#[non_exhaustive]
#[schemars(description = "RFC 6749 §5.1 — the successful access token response.")]
pub struct AccessTokenResponse {
    /// §5.1 REQUIRED — the token itself, a bearer credential until it expires.
    #[schemars(with = "String")]
    pub access_token: Secret<String>,
    /// §5.1 REQUIRED — `Bearer` for RFC 6750.
    pub token_type: String,
    /// §5.1 RECOMMENDED — lifetime in seconds, from the moment of the response.
    pub expires_in: u64,
    /// §5.1 — the scope granted, written space-delimited; empty, the member is
    /// absent, which says the grant is the scope requested.
    // The wire is one string, and the member is optional: the OpenAPI document
    // reads schemas under the deserialize contract, where only a default says so.
    #[schemars(with = "String", default)]
    pub scope: Vec<String>,
}

/// What [`AccessTokenResponse`] puts on the wire, borrowed: the one serializer
/// of the type, reached only through its own response.
#[derive(Serialize)]
struct Wire<'a> {
    access_token: &'a str,
    token_type: &'a str,
    expires_in: u64,
    #[serde(
        skip_serializing_if = "<[String]>::is_empty",
        serialize_with = "nest_rs_authn::scope::space_delimited::serialize"
    )]
    scope: &'a [String],
}

impl AccessTokenResponse {
    /// A `Bearer` token (RFC 6750) living `expires_in` seconds, its scope unstated.
    pub fn bearer(access_token: impl Into<String>, expires_in: u64) -> Self {
        Self {
            access_token: Secret::new(access_token.into()),
            token_type: "Bearer".to_owned(),
            expires_in,
            scope: Vec::new(),
        }
    }

    /// The scope granted, which §5.1 requires whenever it differs from the one
    /// requested.
    pub fn with_scope(mut self, granted: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.scope = granted.into_iter().map(Into::into).collect();
        self
    }
}

impl IntoResponse for AccessTokenResponse {
    fn into_response(self) -> Response {
        let Self {
            access_token,
            token_type,
            expires_in,
            scope,
        } = &self;
        let mut response = Json(Wire {
            access_token: access_token.expose_secret(),
            token_type,
            expires_in: *expires_in,
            scope,
        })
        .into_response();
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
        response
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_http::StatusCode;

    use super::*;

    fn response() -> AccessTokenResponse {
        AccessTokenResponse::bearer("tok_secret", 60)
    }

    /// The OpenAPI component says what the wire type is, not how to return it.
    #[test]
    fn the_schemas_describe_the_wire_types_in_one_sentence() {
        for (schema, sentence) in [
            (
                schemars::schema_for!(AccessTokenRequest),
                "RFC 6749 §4.4.2 — the `client_credentials` access token request.",
            ),
            (
                schemars::schema_for!(AccessTokenResponse),
                "RFC 6749 §5.1 — the successful access token response.",
            ),
        ] {
            assert_eq!(
                schema
                    .get("description")
                    .and_then(serde_json::Value::as_str),
                Some(sentence),
            );
        }
    }

    /// §5.1: `scope` is OPTIONAL when it is the one requested, so a client
    /// reading the published schema must not expect it.
    #[test]
    fn the_schema_lists_scope_as_an_optional_string() {
        let schema = schemars::schema_for!(AccessTokenResponse);

        assert_eq!(
            schema
                .get("properties")
                .and_then(|properties| properties.get("scope"))
                .and_then(|scope| scope.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("string"),
        );
        let mut required: Vec<&str> = schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .expect("the required members are listed")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["access_token", "expires_in", "token_type"]);
    }

    /// §4.4.2: `grant_type` is REQUIRED and `scope` OPTIONAL, which the
    /// published form says with no `Deserialize` to read it off.
    #[test]
    fn the_request_schema_requires_grant_type_alone() {
        let schema = schemars::schema_for!(AccessTokenRequest);

        assert_eq!(
            schema.get("required"),
            Some(&serde_json::json!(["grant_type"]))
        );
    }

    #[test]
    fn debug_never_prints_the_token() {
        let printed = format!("{:?}", response().with_scope(["posts:read"]));

        assert!(!printed.contains("tok_secret"), "{printed}");
        assert!(
            printed.contains("Bearer") && printed.contains("60") && printed.contains("posts:read"),
            "{printed}"
        );
    }

    /// RFC 6749 §5.1: a response carrying a token is never stored.
    #[tokio::test]
    async fn the_response_renders_as_json_that_no_cache_keeps() {
        let rendered = response().into_response();

        assert_eq!(rendered.status(), StatusCode::OK);
        let headers = rendered.headers();
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(headers[header::PRAGMA], "no-cache");
        let media = headers[header::CONTENT_TYPE].to_str().expect("ASCII");
        assert!(media.starts_with("application/json"), "{media}");

        let body = rendered.into_body().into_string().await.expect("a body");
        let body: serde_json::Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(
            body,
            serde_json::json!({
                "access_token": "tok_secret",
                "token_type": "Bearer",
                "expires_in": 60,
            })
        );
    }
}
