use api::ApiModule;
use nest_rs::authn::AuthnConfig;
use nest_rs::testing::{EphemeralDatabase, TestApp};
use poem::http::{StatusCode, header};
use serde_json::json;

use crate::*;

#[tokio::test]
async fn responses_are_gzip_compressed_when_the_client_accepts_it() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<ApiModule>()
        .provide_arc(db.connection())
        .provide(AuthnConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(AUDIENCE.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("ApiModule boots on a compression-enabled transport");
    let _db = db;
    let bearer = format!("Bearer {}", login().await);

    let compressed = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    compressed.assert_status_is_ok();
    compressed.assert_header(header::CONTENT_ENCODING, "gzip");

    let plain = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    plain.assert_status_is_ok();
    plain.assert_header_is_not_exist(header::CONTENT_ENCODING);
}

#[tokio::test]
async fn protected_route_rejects_a_missing_or_bogus_bearer_token() {
    let (_db, app) = boot().await;

    app.http()
        .get("/orgs")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    app.http()
        .get("/orgs")
        .header(header::AUTHORIZATION, "Bearer not-a-real-jwt")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn every_modelled_failure_returns_rfc9457_problem_json() {
    let (_db, app) = boot().await;
    let bearer = format!("Bearer {}", login().await);

    let unauthorized = app.http().get("/orgs").send().await;
    unauthorized.assert_status(StatusCode::UNAUTHORIZED);
    unauthorized.assert_header(header::CONTENT_TYPE, "application/problem+json");
    let body = unauthorized.json().await;
    let problem = body.value().object();
    assert_eq!(problem.get("status").i64(), 401);
    assert_eq!(problem.get("title").string(), "Unauthorized");
    assert!(
        problem.get_opt("type").is_some(),
        "problem carries a type URI"
    );

    let bad_request = app
        .http()
        .get("/users/not-a-uuid")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    bad_request.assert_status(StatusCode::BAD_REQUEST);
    bad_request.assert_header(header::CONTENT_TYPE, "application/problem+json");
    assert_eq!(
        bad_request
            .json()
            .await
            .value()
            .object()
            .get("status")
            .i64(),
        400,
    );

    let not_found = app
        .http()
        .get("/users/018f0000-0000-7000-8000-0000000000ff")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    not_found.assert_status(StatusCode::NOT_FOUND);
    not_found.assert_header(header::CONTENT_TYPE, "application/problem+json");
    assert_eq!(
        not_found.json().await.value().object().get("status").i64(),
        404,
    );

    create_org(&app, &bearer, "Conflict Co").await;
    let conflict = app
        .http()
        .post("/orgs")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "name": "Conflict Co" }))
        .send()
        .await;
    conflict.assert_status(StatusCode::CONFLICT);
    conflict.assert_header(header::CONTENT_TYPE, "application/problem+json");
    let conflict_body = conflict.json().await;
    let conflict_problem = conflict_body.value().object();
    assert_eq!(conflict_problem.get("status").i64(), 409);
    assert_eq!(conflict_problem.get("title").string(), "Conflict");
}

const FOREIGN_MEDIA_TYPE: &str = "text/x-quoted-marker";

#[tokio::test]
async fn a_body_under_another_media_type_answers_415_naming_only_the_expected_type() {
    let (_db, app) = boot().await;
    let bearer = format!(
        "Bearer {}",
        token_with_sub(ORG_ID, "admin", Uuid::now_v7()).await
    );
    let any = Uuid::now_v7();
    let json_bodies = [
        ("POST", "/users".to_owned()),
        ("PUT", format!("/users/{any}")),
        ("POST", "/posts".to_owned()),
        ("PUT", format!("/posts/{any}")),
        ("POST", "/orgs".to_owned()),
        ("PUT", format!("/orgs/{any}")),
        ("POST", "/audio/uploads".to_owned()),
        ("POST", "/audio/transcode".to_owned()),
    ];
    let multipart_bodies = [("POST", "/audio/uploads/direct".to_owned())];
    for (expected, routes) in [
        ("application/json", &json_bodies[..]),
        ("multipart/form-data", &multipart_bodies[..]),
    ] {
        for (method, path) in routes {
            let request = match *method {
                "POST" => app.http().post(path),
                _ => app.http().put(path),
            };
            let resp = request
                .header(header::AUTHORIZATION, &bearer)
                .header(header::CONTENT_TYPE, FOREIGN_MEDIA_TYPE)
                .body("{}")
                .send()
                .await;
            resp.assert_status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            let body = resp.0.into_body().into_string().await.expect("a body");
            assert!(
                !body.contains("x-quoted-marker"),
                "{method} {path} quoted the client's media type: {body}"
            );
            assert!(
                body.contains(expected),
                "{method} {path} names the type it reads: {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_undecodable_value_is_refused_without_quoting_it() {
    let (_db, app) = boot().await;
    let bearer = format!("Bearer {}", login().await);
    let secret = "sk_live_51UndecodableMarker";
    let resp = app
        .http()
        .post("/orgs")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "name": [secret] }))
        .send()
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body = resp.0.into_body().into_string().await.expect("a body");
    assert!(!body.contains(secret), "{body}");
    assert!(body.contains("expected a string"), "{body}");

    let resp = app
        .http()
        .get(format!("/orgs/{secret}"))
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    let body = resp.0.into_body().into_string().await.expect("a body");
    assert!(!body.contains("sk_live"), "{body}");
}
