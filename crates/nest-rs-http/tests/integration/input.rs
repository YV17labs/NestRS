//! `#[input]` carries every derive an HTTP wire DTO needs.
//!
//! The gap this pins: it used to append `Deserialize` + `Validate` +
//! `deny_unknown_fields` but not `JsonSchema`, which is not optional in
//! practice — `#[routes]` documents every `Json<T>` / `Query<T>` argument, so a
//! DTO without it failed to compile against `schema_of` with a trait-bound error
//! that named neither the derive nor the DTO. Two of the docs' own snippets
//! shipped that way. The shorthand exists to absorb exactly this boilerplate.

use nest_rs_http::input;
use nest_rs_http::poem::IntoResponse;
use nest_rs_http::poem::web::Json;
use schemars::JsonSchema;
use validator::Validate;

#[input]
#[derive(Debug)]
struct CreateUser {
    #[validate(length(min = 1))]
    name: String,
}

/// Compile-time: the DTO satisfies each bound a route argument is held to.
fn assert_bounds<T: serde::de::DeserializeOwned + Validate + JsonSchema>() {}

#[test]
fn input_derives_deserialize_validate_and_json_schema() {
    assert_bounds::<CreateUser>();
    // `JsonSchema` is the one that was missing, so assert it produced a real
    // schema rather than only that the bound holds.
    let schema = serde_json::to_value(schemars::schema_for!(CreateUser)).expect("schema");
    assert!(
        schema["properties"].get("name").is_some(),
        "the generated schema describes the DTO's fields: {schema}",
    );
}

#[test]
fn input_rejects_an_unknown_field_at_parse_time() {
    // `deny_unknown_fields`, the other half of the shorthand — pinned alongside
    // so a future edit to the derive list cannot drop it unnoticed.
    let err = serde_json::from_str::<CreateUser>(r#"{"name":"ada","is_admin":true}"#)
        .expect_err("an unknown field is refused");
    assert!(
        err.to_string().contains("is_admin"),
        "the error names the offending field: {err}",
    );
}

/// R9-1: the shorthand also derives `Serialize` — the derive that lets one
/// `#[input]` type be *returned* as `Json<T>`, which is what the docs' response
/// snippets do. It went undocumented, so a reader adding it by hand hit `E0119`
/// on a conflicting impl. Asserted through a real `Json<T>` response rather
/// than a bound, so it pins the behaviour the snippets rely on.
#[test]
fn input_derives_serialize_so_a_dto_can_be_returned_as_json() {
    /// `Json<T>` is only a response when `T: Serialize` — the bound the docs'
    /// `-> Json<GreetReply>` snippets lean on.
    fn assert_returnable<T: serde::Serialize>()
    where
        Json<T>: IntoResponse,
    {
    }
    assert_returnable::<CreateUser>();

    let reply = CreateUser {
        name: "ada".to_owned(),
    };
    let body = serde_json::to_value(&reply).expect("an `#[input]` DTO serializes");
    assert_eq!(body["name"], "ada");
}

#[test]
fn input_validation_rules_still_apply() {
    let parsed: CreateUser = serde_json::from_str(r#"{"name":""}"#).expect("parses");
    assert!(
        parsed.validate().is_err(),
        "`#[validate(length(min = 1))]` is live on an `#[input]` DTO",
    );
}
