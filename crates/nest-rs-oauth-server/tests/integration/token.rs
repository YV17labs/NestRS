//! RFC 6749 §4.4.2 and §5.1 on the wire.
//!
//! The point of holding these shapes in the framework is that every issuer
//! spells them the same, so what is asserted here is the **spelling**: the
//! member names the specification fixes, and the two `serde` behaviours a
//! conforming endpoint depends on. The transport encoding — `Form` at the token
//! endpoint — is poem's, not this type's, so it is asserted where it is applied.

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

/// §5.2 obliges the endpoint to answer an unknown grant with
/// `unsupported_grant_type`, which it can only do if the value reaches it.
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
