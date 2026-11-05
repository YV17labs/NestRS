use api::ApiModule;
use features::Role;
use features::testing::{API, ASSISTANT, with_claim};
use nest_rs::http::{Method, StatusCode, header};
use nest_rs::schedule::Scheduler;
use nest_rs::testing::{HeadlessApp, TestApp, TestResponse};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::openapi::served_document;
use crate::{ORG_ID, boot, claims, client};

const ROOT_OPERATIONS: [(&str, &str, &str); 13] = [
    (
        "Mutation",
        "createOrg",
        "mutation { createOrg(input: { name: \"x\" }) { id } }",
    ),
    (
        "Mutation",
        "createUser",
        "mutation { createUser(input: { name: \"x\", email: \"x@x.test\" }) { id } }",
    ),
    (
        "Mutation",
        "deleteOrg",
        "mutation { deleteOrg(id: \"$ID\") }",
    ),
    (
        "Mutation",
        "deleteUser",
        "mutation { deleteUser(id: \"$ID\") }",
    ),
    (
        "Mutation",
        "publishPost",
        "mutation { publishPost(id: \"$ID\") { id } }",
    ),
    (
        "Mutation",
        "updateOrg",
        "mutation { updateOrg(id: \"$ID\", input: { name: \"x\" }) { id } }",
    ),
    (
        "Mutation",
        "updateUser",
        "mutation { updateUser(id: \"$ID\", input: { name: \"x\", email: \"x@x.test\" }) { id } }",
    ),
    ("Query", "org", "{ org(id: \"$ID\") { id } }"),
    ("Query", "orgs", "{ orgs { id } }"),
    ("Query", "post", "{ post(id: \"$ID\") { id } }"),
    ("Query", "posts", "{ posts { id } }"),
    ("Query", "user", "{ user(id: \"$ID\") { id } }"),
    ("Query", "users", "{ users { id } }"),
];

fn root_fields<'a>(schema: &'a str, root: &str) -> Vec<&'a str> {
    let opening = format!("type {root} {{");
    schema
        .lines()
        .skip_while(|line| *line != opening)
        .skip(1)
        .take_while(|line| *line != "}")
        .map(|line| {
            line.trim()
                .split(['(', ':'])
                .next()
                .expect("a field has a name")
        })
        .collect()
}

#[test]
fn the_anonymous_table_names_every_root_operation_of_the_schema() {
    let schema = include_str!("../../schema.graphql");

    for root in ["Mutation", "Query"] {
        let listed: Vec<&str> = ROOT_OPERATIONS
            .iter()
            .filter(|(of, _, _)| *of == root)
            .map(|(_, field, _)| *field)
            .collect();
        assert_eq!(listed, root_fields(schema, root), "{root}");
    }
    assert_eq!(
        root_fields(schema, "Subscription"),
        ["postPublished"],
        "a subscription is proved in `subscription`",
    );
}

#[tokio::test]
async fn an_anonymous_caller_is_refused_on_every_route_the_api_documents_but_its_probes() {
    let app = boot().await;
    let document = served_document(&app).await;
    let id = Uuid::nil().to_string();

    let mut refused = Vec::new();
    for (template, item) in document["paths"].as_object().expect("paths") {
        if template.starts_with("/health/") {
            continue;
        }
        let path = template
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    id.as_str()
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        for verb in item.as_object().expect("a path item").keys() {
            let method = Method::from_bytes(verb.to_uppercase().as_bytes()).expect("an HTTP verb");
            let answer = app.http().request(method, path.clone()).send().await;
            assert_eq!(
                answer.0.status(),
                StatusCode::UNAUTHORIZED,
                "{verb} {template}",
            );
            refused.push(format!("{verb} {template}"));
        }
    }

    for reached in [
        "get /users",
        "post /posts",
        "get /orgs",
        "get /notifications",
        "post /audio/transcode",
    ] {
        assert!(
            refused.iter().any(|route| route == reached),
            "{reached} in {refused:?}",
        );
    }
}

#[tokio::test]
async fn an_anonymous_caller_is_refused_on_every_root_operation() {
    let app = boot().await;
    let id = Uuid::nil().to_string();

    for (_, field, operation) in ROOT_OPERATIONS {
        let answer = app
            .http()
            .post("/graphql")
            .body_json(&json!({ "query": operation.replace("$ID", &id) }))
            .send()
            .await;
        answer.assert_status_is_ok();
        let body = answer.json().await.value().deserialize::<Value>();
        assert_eq!(
            body["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
            "{field}: {body}",
        );
        assert!(body["data"][field].is_null(), "{field}: {body}");
    }
}

#[tokio::test]
async fn the_api_requires_its_own_audience_read_from_its_own_file() {
    let app = boot().await;
    let for_resource = |resource: &str| {
        app.bearer(&with_claim(
            claims(ORG_ID, Role::Admin, client()),
            "aud",
            resource,
        ))
    };

    list_users(&app, &for_resource(ASSISTANT))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    list_users(&app, &for_resource(API))
        .await
        .assert_status_is_ok();
}

async fn list_users(app: &TestApp, authorization: &str) -> TestResponse {
    app.http()
        .get("/users")
        .header(header::AUTHORIZATION, authorization)
        .send()
        .await
}

#[tokio::test]
async fn api_app_binds_the_lock_its_one_replica_transcode_seed_claims_through() {
    let api = HeadlessApp::builder()
        .module::<ApiModule>()
        .ephemeral_database::<migrations::Migrator>()
        .build_headless()
        .await
        .expect("ApiModule boots against a throwaway database and Redis");
    let scheduler = api.spawn_transport(Scheduler::new()).await.expect(
        "the scheduler configures: the transcode seed declared replicas = \"one\" has a lock",
    );
    scheduler
        .shutdown()
        .await
        .expect("the scheduler stops cleanly");
}
