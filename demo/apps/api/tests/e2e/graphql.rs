use poem::http::header;
use serde_json::json;
use uuid::Uuid;

use super::harness::*;

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
        .body_json(&json!({ "query": "{ orgs { id users { email } } }" }))
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
        for u in org.object().get("users").array().iter() {
            seen.push(u.object().get("email").string().to_owned());
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
async fn has_many_relation_load_is_capped_at_relation_load_cap() {
    use sea_orm::ConnectionTrait;

    let (db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &admin, "Fanout").await;
    let token = format!("Bearer {}", token_for(&org, "admin").await);

    let author_resp = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token)
        .body_json(&json!({ "name": "Author", "email": "fanout-author@rel.test" }))
        .send()
        .await;
    author_resp.assert_status_is_ok();
    let author = author_resp
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    let seeded = nest_rs_seaorm::RELATION_LOAD_CAP + 5;
    let rows: Vec<String> = (0..seeded)
        .map(|i| format!("('{}','{org}','{author}','t{i}','b{i}')", Uuid::now_v7()))
        .collect();
    db.connection()
        .execute_unprepared(&format!(
            "INSERT INTO post (id, org_id, author_id, title, body) VALUES {}",
            rows.join(", "),
        ))
        .await
        .expect("bulk insert posts");

    let resp = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token)
        .body_json(&json!({ "query": "{ orgs { id posts { id } } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "graphql response must not contain errors",
    );
    let loaded = body
        .value()
        .object()
        .get("data")
        .object()
        .get("orgs")
        .array()
        .iter()
        .find(|o| o.object().get("id").string() == org.as_str())
        .expect("the seeded org is present in the response")
        .object()
        .get("posts")
        .array()
        .iter()
        .count() as u64;
    assert_eq!(
        loaded,
        nest_rs_seaorm::RELATION_LOAD_CAP,
        "an exposed has_many load is bounded at RELATION_LOAD_CAP, not the {seeded} seeded",
    );
}
