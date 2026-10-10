//! The token endpoint's **successful** exchange — RFC 6749 §4.4.2's request and
//! §5.1's response.

use std::fmt;

use poem::http::{HeaderValue, header};
use poem::web::Json;
use poem::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// RFC 6749 §4.4.2 — the `client_credentials` access token request.
///
/// `grant_type` is a `String`: §5.2 answers an unknown grant with
/// [`TokenError::UnsupportedGrant`](crate::TokenError::UnsupportedGrant), which an
/// enum would turn into a deserialization error the issuer never sees.
#[derive(Debug, Deserialize, JsonSchema)]
// schemars would otherwise publish this whole rustdoc as the schema's description.
#[schemars(description = "RFC 6749 §4.4.2 — the `client_credentials` access token request.")]
pub struct AccessTokenRequest {
    /// The grant being exercised — `client_credentials` at this endpoint.
    pub grant_type: String,
    /// §3.3 space-delimited scope list. Absent ⇒ the issuer's own default.
    #[serde(default)]
    pub scope: Option<String>,
}

/// RFC 6749 §5.1 — the successful access token response.
///
/// A handler returns it bare: it renders as its JSON with §5.1's
/// `Cache-Control: no-store` and `Pragma: no-cache`, which `Json<Self>` would
/// drop. `#[api(response = …)]` names it for the OpenAPI document, which reads
/// a body's type off a `Json<T>` return. `Debug` redacts the token.
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
///     async fn login(&self) -> poem::Result<AccessTokenResponse> {
///         Ok(sign_in())
///     }
/// }
/// ```
#[derive(Serialize, JsonSchema)]
#[schemars(description = "RFC 6749 §5.1 — the successful access token response.")]
pub struct AccessTokenResponse {
    /// §5.1 REQUIRED — the token itself.
    pub access_token: String,
    /// §5.1 REQUIRED — `Bearer` for RFC 6750.
    pub token_type: String,
    /// §5.1 RECOMMENDED — lifetime in seconds, from the moment of the response.
    pub expires_in: u64,
    /// §5.1 — the scope granted, written space-delimited; empty, the member is
    /// absent, which says the grant is the scope requested.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "nest_rs_authn::scope::space_delimited::serialize"
    )]
    // The wire is one string, and the member is optional: the OpenAPI document
    // reads schemas under the deserialize contract, where only a default says so.
    #[schemars(with = "String", default)]
    pub scope: Vec<String>,
}

impl AccessTokenResponse {
    /// A `Bearer` token (RFC 6750) living `expires_in` seconds, its scope unstated.
    pub fn bearer(access_token: impl Into<String>, expires_in: u64) -> Self {
        Self {
            access_token: access_token.into(),
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

impl fmt::Debug for AccessTokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Destructured, so a member added later is a compile error here until
        // it says whether it is a credential.
        let Self {
            access_token: _,
            token_type,
            expires_in,
            scope,
        } = self;
        f.debug_struct("AccessTokenResponse")
            .field("access_token", &"<redacted>")
            .field("token_type", token_type)
            .field("expires_in", expires_in)
            .field("scope", scope)
            .finish()
    }
}

impl IntoResponse for AccessTokenResponse {
    fn into_response(self) -> Response {
        Json(self)
            .with_header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
            .with_header(header::PRAGMA, HeaderValue::from_static("no-cache"))
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use poem::http::{StatusCode, header};

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
