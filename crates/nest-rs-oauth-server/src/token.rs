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
/// ```
/// # use nest_rs_http::{controller, routes};
/// use nest_rs_oauth_server::AccessTokenResponse;
/// # fn sign_in() -> AccessTokenResponse {
/// #     AccessTokenResponse { access_token: "t".into(), token_type: "Bearer".into(), expires_in: 60 }
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
}

impl fmt::Debug for AccessTokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Destructured, so a member added later is a compile error here until
        // it says whether it is a credential.
        let Self {
            access_token: _,
            token_type,
            expires_in,
        } = self;
        f.debug_struct("AccessTokenResponse")
            .field("access_token", &"<redacted>")
            .field("token_type", token_type)
            .field("expires_in", expires_in)
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
        AccessTokenResponse {
            access_token: "tok_secret".into(),
            token_type: "Bearer".into(),
            expires_in: 60,
        }
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

    #[test]
    fn debug_never_prints_the_token() {
        let printed = format!("{:?}", response());

        assert!(!printed.contains("tok_secret"), "{printed}");
        assert!(
            printed.contains("Bearer") && printed.contains("60"),
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
