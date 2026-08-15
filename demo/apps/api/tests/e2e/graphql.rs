use poem::http::header;
use serde_json::json;
use uuid::Uuid;

use crate::*;

#[tokio::test]
async fn graphql_requires_a_jwt_and_scopes_to_the_callers_org() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &admin, "GqlAcme").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!(
        "Bearer {}",
        token_for(&create_org(&app, &admin, "GqlGlobex").await, "admin").await
    );

    let created = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Gql Ada", "email": "gqlada@acme.test" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let user_a = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    let query = json!({ "query": "{ users { name } }" });

    let anon = app.http().post("/graphql").body_json(&query).send().await;
    anon.assert_status_is_ok();
    assert!(
        anon.json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "an anonymous GraphQL query is rejected",
    );

    let b = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_b)
        .body_json(&query)
        .send()
        .await;
    b.assert_status_is_ok();
    let b_users = b.json().await;
    let b_names: Vec<String> = b_users
        .value()
        .object()
        .get("data")
        .object()
        .get("users")
        .array()
        .iter()
        .map(|u| u.object().get("name").string().to_owned())
        .collect();
    assert!(
        b_names.is_empty(),
        "org B sees no users in GraphQL: {b_names:?}"
    );

    let a = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&query)
        .send()
        .await;
    a.assert_status_is_ok();
    let a_users = a.json().await;
    let a_names: Vec<String> = a_users
        .value()
        .object()
        .get("data")
        .object()
        .get("users")
        .array()
        .iter()
        .map(|u| u.object().get("name").string().to_owned())
        .collect();
    assert_eq!(a_names, vec!["Gql Ada".to_string()]);

    let by_id = json!({ "query": format!("{{ user(id: \"{user_a}\") {{ name }} }}") });
    let a_one = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&by_id)
        .send()
        .await;
    a_one.assert_status_is_ok();
    assert_eq!(
        a_one
            .json()
            .await
            .value()
            .object()
            .get("data")
            .object()
            .get("user")
            .object()
            .get("name")
            .string(),
        "Gql Ada",
    );
    let b_one = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_b)
        .body_json(&by_id)
        .send()
        .await;
    b_one.assert_status_is_ok();
    assert!(
        b_one
            .json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "org B is forbidden org A's user by id",
    );
}

#[tokio::test]
async fn graphql_auto_resolved_relations_respect_ability_scope() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &admin, "RelA").await;
    let org_b = create_org(&app, &admin, "RelB").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!("Bearer {}", token_for(&org_b, "admin").await);

    for (tok, email) in [
        (&token_a, "ada@rel.test"),
        (&token_a, "bea@rel.test"),
        (&token_b, "leak@rel.test"),
    ] {
        app.http()
            .post("/users")
            .header(header::AUTHORIZATION, tok)
            .body_json(&json!({ "name": "Twin", "email": email }))
            .send()
            .await
            .assert_status_is_ok();
    }

    let resp = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "query": "{ users { id org { id } } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "graphql response must not contain errors",
    );
    let users_a = body
        .value()
        .object()
        .get("data")
        .object()
        .get("users")
        .array();
    assert!(
        users_a.iter().count() >= 2,
        "org A must see its two seeded users (got {})",
        users_a.iter().count(),
    );
    for u in users_a.iter() {
        let org_id = u.object().get("org").object().get("id").string();
        assert_eq!(
            org_id, org_a,
            "auto-resolved org must be caller's: {org_id}"
        );
    }

    let resp = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({
            "query": "{ orgs { id users { edges { node { email } } } } }"
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "graphql response must not contain errors",
    );
    let mut seen: Vec<String> = Vec::new();
    for org in body
        .value()
        .object()
        .get("data")
        .object()
        .get("orgs")
        .array()
        .iter()
    {
        for edge in org
            .object()
            .get("users")
            .object()
            .get("edges")
            .array()
            .iter()
        {
            seen.push(
                edge.object()
                    .get("node")
                    .object()
                    .get("email")
                    .string()
                    .to_owned(),
            );
        }
    }
    assert!(
        seen.iter().any(|e| e == "ada@rel.test"),
        "org A's own users must surface through the HasMany resolver: {seen:?}",
    );
    assert!(
        !seen.contains(&"leak@rel.test".to_string()),
        "org B's user must not leak through Org.users: {seen:?}",
    );
}

#[tokio::test]
async fn every_parent_gets_its_own_page_of_an_auto_resolved_has_many() {
    use sea_orm::ConnectionTrait;

    let (db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &admin, "Fanout").await;
    let token = format!("Bearer {}", token_for(&org, "admin").await);

    let authors = [
        create_user(&app, &token, "Ann", "fanout-ann@rel.test").await,
        create_user(&app, &token, "Bo", "fanout-bo@rel.test").await,
        create_user(&app, &token, "Cy", "fanout-cy@rel.test").await,
    ];
    let per_author = 25_u64;
    let page = 10_u64;

    let mut rows: Vec<String> = Vec::new();
    for author in &authors {
        for i in 0..per_author {
            rows.push(format!(
                "('{}','{org}','{author}','t{i}','b{i}')",
                Uuid::now_v7()
            ));
        }
    }
    db.connection()
        .execute_unprepared(&format!(
            "INSERT INTO post (id, org_id, author_id, title, body) VALUES {}",
            rows.join(", "),
        ))
        .await
        .expect("bulk insert posts");

    let first_page = graphql(
        &app,
        &token,
        &format!(
            "{{ users {{ id posts(first: {page}) {{ \
               edges {{ cursor node {{ id }} }} \
               pageInfo {{ hasNextPage hasPreviousPage endCursor }} }} }} }}"
        ),
    )
    .await;
    assert!(
        first_page.get("errors").is_none(),
        "a paginated relation must resolve: {first_page}"
    );

    let mut walked: Vec<String> = Vec::new();
    for author in &authors {
        let connection = connection_of(&first_page, author);
        let ids = node_ids(connection);
        assert_eq!(
            ids.len() as u64,
            page,
            "each parent gets its own page, not a share of one global limit: {first_page}",
        );
        assert_eq!(connection["pageInfo"]["hasNextPage"], json!(true));
        assert_eq!(connection["pageInfo"]["hasPreviousPage"], json!(false));
        walked.extend(ids);
    }
    assert_eq!(
        walked.len(),
        walked
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        "no post is served to two parents",
    );

    let cursor = connection_of(&first_page, &authors[1])["pageInfo"]["endCursor"]
        .as_str()
        .expect("a page with more to come carries an endCursor")
        .to_owned();
    let second_page = graphql(
        &app,
        &token,
        &format!(
            "{{ users {{ id posts(first: {page}, after: \"{cursor}\") {{ \
               edges {{ node {{ id }} }} pageInfo {{ hasPreviousPage }} }} }} }}"
        ),
    )
    .await;
    let second = connection_of(&second_page, &authors[1]);
    let second_ids = node_ids(second);
    assert_eq!(
        second_ids.len() as u64,
        page,
        "a per-parent window is expressible: the middle author has a full second page, \
         where a batch-shaped loader left every late-sorting parent looking childless: \
         {second_page}",
    );
    assert_eq!(second["pageInfo"]["hasPreviousPage"], json!(true));
    let firsts = node_ids(connection_of(&first_page, &authors[1]));
    assert!(
        second_ids.iter().all(|id| !firsts.contains(id)),
        "the cursor advances the window rather than replaying it",
    );
}

fn connection_of<'a>(body: &'a serde_json::Value, user: &str) -> &'a serde_json::Value {
    body["data"]["users"]
        .as_array()
        .unwrap_or_else(|| panic!("users is a list: {body}"))
        .iter()
        .find(|u| u["id"] == json!(user))
        .unwrap_or_else(|| panic!("user {user} is in the response: {body}"))
        .get("posts")
        .unwrap_or_else(|| panic!("the user carries its posts connection: {body}"))
}

fn node_ids(connection: &serde_json::Value) -> Vec<String> {
    connection["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("a connection carries edges: {connection}"))
        .iter()
        .map(|e| e["node"]["id"].as_str().expect("a node id").to_owned())
        .collect()
}

#[tokio::test]
async fn graphql_serves_a_member_the_columns_their_field_grant_allows() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &admin, "GqlFields").await;
    let admin_org = format!("Bearer {}", token_for(&org, "admin").await);
    let member = format!("Bearer {}", token_for(&org, "user").await);
    create_user(&app, &admin_org, "Gql Grace", "gqlgrace@fields.test").await;

    let granted = graphql(&app, &member, "{ users { id name } }").await;
    assert!(
        granted.get("errors").is_none(),
        "a query asking only for granted columns must be served: {granted}",
    );
    let names: Vec<&str> = granted["data"]["users"]
        .as_array()
        .unwrap_or_else(|| panic!("users is a list: {granted}"))
        .iter()
        .map(|u| u["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, vec!["Gql Grace"], "{granted}");

    let refused = graphql(&app, &member, "{ users { id name email } }").await;
    assert_eq!(
        refused["data"],
        serde_json::Value::Null,
        "no row may ship with a column outside the field grant: {refused}",
    );
    assert_eq!(
        refused["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{refused}",
    );
    assert_eq!(
        refused["errors"][0]["extensions"]["fields"],
        serde_json::json!(["email"]),
        "the denial names the columns it refused, as a list: {refused}",
    );

    let admin_view = graphql(&app, &admin_org, "{ users { id name email } }").await;
    assert!(admin_view.get("errors").is_none(), "{admin_view}");
    assert_eq!(
        admin_view["data"]["users"][0]["email"], "gqlgrace@fields.test",
        "{admin_view}",
    );
}

async fn graphql(app: &nest_rs::testing::TestApp, bearer: &str, query: &str) -> serde_json::Value {
    let resp = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, bearer)
        .body_json(&json!({ "query": query }))
        .send()
        .await;
    resp.assert_status_is_ok();
    serde_json::to_value(resp.json().await).expect("a GraphQL response is JSON")
}

#[tokio::test]
async fn graphql_validation_errors_name_the_offending_fields() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    let rejected = graphql(
        &app,
        &admin,
        r#"mutation { createOrg(input: {name: ""}) { id } }"#,
    )
    .await;

    assert_eq!(rejected["data"], serde_json::Value::Null, "{rejected}");
    let errors = &rejected["errors"][0]["extensions"]["errors"];
    assert!(
        errors.is_object(),
        "the rejection must carry the field errors under `extensions.errors`: {rejected}",
    );
    assert!(
        errors.get("name").is_some(),
        "`name` was rejected but is not named: {rejected}",
    );
    assert_eq!(
        rejected["errors"][0]["message"], "validation failed",
        "{rejected}"
    );
    assert!(
        !rejected.to_string().contains("Validation error:"),
        "`validator`'s raw debug payload must not reach the wire: {rejected}",
    );
}
