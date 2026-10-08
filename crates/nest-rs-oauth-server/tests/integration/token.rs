//! RFC 6749 §4.4.2 and §5.1 on the wire.

use nest_rs_oauth_server::{AccessTokenRequest, AccessTokenResponse};

#[test]
fn the_response_carries_exactly_the_members_5_1_marks_required() {
    let body = serde_json::to_value(AccessTokenResponse {
        access_token: "t".into(),
        token_type: "Bearer".into(),
        expires_in: 3600,
    })
    .expect("serializes");

    let object = body.as_object().expect("a JSON object");
    let mut members: Vec<&str> = object.keys().map(String::as_str).collect();
    members.sort_unstable();
    assert_eq!(members, ["access_token", "expires_in", "token_type"]);
}

#[test]
fn a_request_omitting_scope_deserializes_because_3_3_makes_it_optional() {
    let request: AccessTokenRequest =
        serde_json::from_str(r#"{"grant_type":"client_credentials"}"#).expect("body");

    assert_eq!(request.grant_type, "client_credentials");
    assert_eq!(request.scope, None);
}

#[test]
fn an_unknown_grant_reaches_the_issuer_rather_than_failing_to_deserialize() {
    let request: AccessTokenRequest =
        serde_json::from_str(r#"{"grant_type":"urn:ietf:params:oauth:grant-type:device_code"}"#)
            .expect("body");

    assert_eq!(
        request.grant_type,
        "urn:ietf:params:oauth:grant-type:device_code"
    );
}
