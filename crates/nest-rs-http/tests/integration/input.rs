//! `#[input]` carries every derive an HTTP wire DTO needs.

use nest_rs_core::{Secret, input};
use nest_rs_http::{IntoResponse, Json};
use schemars::JsonSchema;
use validator::Validate;

#[input]
#[derive(Debug)]
struct CreateUser {
    #[validate(length(min = 1))]
    name: String,
}

fn assert_bounds<T: serde::de::DeserializeOwned + Validate + JsonSchema>() {}

#[test]
fn input_derives_deserialize_validate_and_json_schema() {
    assert_bounds::<CreateUser>();
    let schema = serde_json::to_value(schemars::schema_for!(CreateUser)).expect("schema");
    assert!(
        schema["properties"].get("name").is_some(),
        "the generated schema describes the DTO's fields: {schema}",
    );
}

#[test]
fn input_rejects_an_unknown_field_at_parse_time() {
    let err = serde_json::from_str::<CreateUser>(r#"{"name":"ada","is_admin":true}"#)
        .expect_err("an unknown field is refused");
    assert!(
        err.to_string().contains("is_admin"),
        "the error names the offending field: {err}",
    );
}

#[test]
fn input_derives_serialize_so_a_dto_can_be_returned_as_json() {
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

#[input]
struct SignIn {
    #[validate(length(min = 8))]
    #[schemars(with = "String")]
    password: Secret<String>,
}

#[test]
fn input_holds_a_secret_its_rules_judge_its_schema_names_and_no_output_prints() {
    let signed_in: SignIn = serde_json::from_str(r#"{"password":"longenough"}"#).expect("parses");
    assert_eq!(signed_in.password.expose_secret(), "longenough");
    assert!(signed_in.validate().is_ok());
    let written = serde_json::to_string(&signed_in).expect("serializes");
    assert!(!written.contains("longenough"), "{written}");

    let schema = serde_json::to_value(schemars::schema_for!(SignIn)).expect("schema");
    assert_eq!(
        schema["properties"]["password"],
        serde_json::json!({ "type": "string", "minLength": 8 }),
        "the document reads as a `String` under the same rule would",
    );

    let too_short: SignIn = serde_json::from_str(r#"{"password":"hunter2"}"#).expect("parses");
    let refusal = too_short.validate().expect_err("seven characters");
    let refusal = serde_json::to_string(&refusal).expect("serializes");
    assert!(!refusal.contains("hunter2"), "{refusal}");
}
