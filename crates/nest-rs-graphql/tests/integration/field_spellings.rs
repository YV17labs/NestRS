//! A field error names a field as the schema spells it, so a client matches a
//! key of `extensions.errors` to the field it sent: under the rename rule its
//! input declares, on every path a field map reaches the wire by.

use async_graphql::futures_util::stream::{self, Stream};
use async_graphql::{InputObject, SimpleObject};
use nest_rs_core::code;
use nest_rs_core::{Problem, module};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_testing::TestApp;
use serde_json::{Value, json};
use validator::Validate;

#[derive(InputObject, Validate)]
struct CreateBadge {
    #[validate(length(min = 1))]
    display_name: String,
    #[validate(nested)]
    home_address: BadgeAddress,
}

#[derive(InputObject, Validate)]
struct BadgeAddress {
    #[validate(length(min = 1))]
    zip_code: String,
}

#[derive(InputObject, Validate)]
#[graphql(rename_fields = "snake_case")]
struct LegacyBadge {
    #[validate(length(min = 1))]
    legacy_name: String,
}

#[derive(InputObject, Validate)]
struct BadgeFilter {
    #[validate(length(min = 1))]
    name_prefix: String,
}

#[derive(SimpleObject)]
#[graphql(complex)]
struct Badge {
    id: i32,
}

#[resolver]
struct BadgesResolver;

#[operations]
impl BadgesResolver {
    #[query]
    #[public]
    async fn badge(&self) -> Badge {
        Badge { id: 1 }
    }

    #[mutation]
    #[public]
    async fn create_badge(&self, input: CreateBadge) -> bool {
        let CreateBadge { .. } = input;
        true
    }

    #[mutation]
    #[public]
    async fn create_legacy_badge(&self, input: LegacyBadge) -> bool {
        let LegacyBadge { .. } = input;
        true
    }

    /// Answers what a `#[crud]` write answers when a unique key refuses it.
    #[mutation]
    #[public]
    async fn take_badge(&self, input: CreateBadge) -> Result<bool, Problem> {
        let CreateBadge { .. } = input;
        Err(Problem::new(409, code::CONFLICT).with_errors(json!({
            "display_name": [{ "code": "unique" }],
            "not_a_field_of_the_schema": [{ "code": "unique" }],
        })))
    }

    #[subscription]
    #[public]
    async fn watch_badge(&self, input: CreateBadge) -> impl Stream<Item = i32> {
        let CreateBadge { .. } = input;
        stream::empty()
    }

    #[entity]
    #[public]
    async fn find_badge_by_id(&self, id: i32) -> Result<Badge, Problem> {
        let _ = id;
        Err(Problem::new(409, code::CONFLICT).with_errors(json!({
            "display_name": [{ "code": "unique" }],
        })))
    }

    #[field_resolver]
    async fn matching(&self, parent: &Badge, filter: BadgeFilter) -> i32 {
        let BadgeFilter { .. } = filter;
        parent.id
    }
}

#[module(imports = [GraphqlModule::for_root(GraphqlConfig {
    federation: true,
    federation_sdl: nest_rs_config::Surface::Open,
    ..GraphqlConfig::default()
})], providers = [BadgesResolver])]
struct BadgesApp;

/// The first error's `extensions.errors` for `query`.
async fn field_errors(query: &str) -> Value {
    let app = TestApp::for_module::<BadgesApp>()
        .await
        .expect("the schema boots");
    let body = crate::graphql_answer(&app, query).await;
    body["errors"][0]["extensions"]["errors"].clone()
}

#[tokio::test]
async fn a_validation_failure_names_a_multi_word_field_as_the_schema_spells_it() {
    let errors = field_errors(
        r#"mutation { createBadge(input: { displayName: "", homeAddress: { zipCode: "" } }) }"#,
    )
    .await;

    assert_eq!(errors["displayName"][0]["code"], "length", "{errors}");
    assert_eq!(
        errors["homeAddress"]["zipCode"][0]["code"], "length",
        "a nested input's fields too: {errors}"
    );
    assert!(errors.get("display_name").is_none(), "{errors}");
}

#[tokio::test]
async fn a_problems_field_map_names_the_field_as_the_schema_spells_it() {
    let errors = field_errors(
        r#"mutation { takeBadge(input: { displayName: "x", homeAddress: { zipCode: "x" } }) }"#,
    )
    .await;

    assert_eq!(
        errors,
        json!({
            "displayName": [{ "code": "unique" }],
            "not_a_field_of_the_schema": [{ "code": "unique" }],
        }),
        "what no input spells stays as it was said",
    );
}

#[tokio::test]
async fn an_input_under_another_rename_rule_keeps_the_spelling_it_declares() {
    let errors =
        field_errors(r#"mutation { createLegacyBadge(input: { legacy_name: "" }) }"#).await;

    assert_eq!(errors["legacy_name"][0]["code"], "length", "{errors}");
}

#[tokio::test]
async fn a_field_resolvers_validation_names_the_field_as_the_schema_spells_it() {
    let errors = field_errors(r#"{ badge { matching(filter: { namePrefix: "" }) } }"#).await;

    assert_eq!(errors["namePrefix"][0]["code"], "length", "{errors}");
}

#[tokio::test]
async fn a_subscriptions_opening_names_the_field_as_the_schema_spells_it() {
    let app = TestApp::for_module::<BadgesApp>()
        .await
        .expect("the schema boots");
    let mut socket = app.graphql_socket().open();
    socket.connect().await;

    socket.subscribe(
        "badge",
        r#"subscription { watchBadge(input: { displayName: "", homeAddress: { zipCode: "" } }) }"#,
    );

    let answer = socket
        .next_message()
        .await
        .expect("the opening is answered");
    let errors = &answer["payload"]["errors"][0]["extensions"]["errors"];
    assert_eq!(errors["displayName"][0]["code"], "length", "{answer}");
    assert_eq!(
        errors["homeAddress"]["zipCode"][0]["code"], "length",
        "{answer}"
    );
    assert!(errors.get("display_name").is_none(), "{answer}");
}

#[tokio::test]
async fn an_entitys_field_map_names_the_field_as_the_schema_spells_it() {
    let errors = field_errors(
        r#"{ _entities(representations: [{ __typename: "Badge", id: 1 }]) { ... on Badge { id } } }"#,
    )
    .await;

    assert_eq!(errors["displayName"][0]["code"], "unique", "{errors}");
    assert!(errors.get("display_name").is_none(), "{errors}");
}
