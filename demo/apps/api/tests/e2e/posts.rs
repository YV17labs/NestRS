//! Posts over GraphQL and the publish -> notification worker flow.

use std::time::Duration;

use features::notifications::NotificationsQueueModule;
use nest_rs_core::module;
use nest_rs_redis::{QueueModule, QueueWorker, QueueWorkerModule};
use nest_rs_seaorm::DatabaseModule;
use nest_rs_testing::TestApp;
use poem::http::header;
use serde_json::json;
use uuid::Uuid;

use super::harness::*;

#[tokio::test]
async fn posts_graphql_scopes_reads_and_publish_transitions() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "PostAcme").await;
    let org_b = create_org(&app, &bootstrap, "PostGlobex").await;

    let admin_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let author_id =
        Uuid::parse_str(&create_user(&app, &admin_a, "Author", "author@postacme.test").await)
            .expect("valid user uuid");
    let author_a = format!(
        "Bearer {}",
        token_with_sub(&org_a, "admin", author_id).await
    );
    let admin_b = format!("Bearer {}", token_for(&org_b, "admin").await);

    let post_a = create_post(&app, &author_a, "Launch", "Big news").await;

    let list = json!({ "query": "{ posts { id status } }" });

    let b = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &admin_b)
        .body_json(&list)
        .send()
        .await;
    b.assert_status_is_ok();
    let b_body = b.json().await;
    assert!(
        b_body.value().object().get_opt("errors").is_none(),
        "org B list must not error",
    );
    assert!(
        b_body
            .value()
            .object()
            .get("data")
            .object()
            .get("posts")
            .array()
            .iter()
            .next()
            .is_none(),
        "org B sees no posts of org A",
    );

    let a = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&list)
        .send()
        .await;
    a.assert_status_is_ok();
    let a_body = a.json().await;
    let a_status = a_body
        .value()
        .object()
        .get("data")
        .object()
        .get("posts")
        .array()
        .iter()
        .find(|p| p.object().get("id").string() == post_a.as_str())
        .expect("org A sees its own post")
        .object()
        .get("status")
        .string()
        .to_owned();
    assert_eq!(a_status, "DRAFT", "a freshly created post is a draft");

    let publish = |id: &str| json!({ "query": format!("mutation {{ publishPost(id: \"{id}\") {{ id status }} }}") });

    let denied = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &admin_b)
        .body_json(&publish(&post_a))
        .send()
        .await;
    denied.assert_status_is_ok();
    assert!(
        denied
            .json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "org B is forbidden publishing org A's post",
    );

    let published = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&publish(&post_a))
        .send()
        .await;
    published.assert_status_is_ok();
    let pub_body = published.json().await;
    assert!(
        pub_body.value().object().get_opt("errors").is_none(),
        "publish must not error",
    );
    assert_eq!(
        pub_body
            .value()
            .object()
            .get("data")
            .object()
            .get("publishPost")
            .object()
            .get("status")
            .string(),
        "PUBLISHED",
    );

    // Republishing an already-published post is a conflict — the invariant now
    // lives in `PostsService::publish`, so GraphQL enforces it like HTTP.
    let republished = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&publish(&post_a))
        .send()
        .await;
    republished.assert_status_is_ok();
    assert!(
        republished
            .json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "republishing an already-published post must conflict",
    );

    let by_id = json!({ "query": format!("{{ post(id: \"{post_a}\") {{ status }} }}") });
    let again = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&by_id)
        .send()
        .await;
    again.assert_status_is_ok();
    assert_eq!(
        again
            .json()
            .await
            .value()
            .object()
            .get("data")
            .object()
            .get("post")
            .object()
            .get("status")
            .string(),
        "PUBLISHED",
    );
}

#[module(
    imports = [
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        QueueWorkerModule,
        NotificationsQueueModule,
    ],
)]
struct NotificationsWorkerHarness;

#[tokio::test]
async fn publishing_a_post_notifies_the_org_through_the_worker() {
    let (db, app) = boot().await;

    let worker = TestApp::builder()
        .module::<NotificationsWorkerHarness>()
        .provide_arc(db.connection())
        .build_headless()
        .await
        .expect("the notifications worker boots against the ephemeral DB and Redis");
    let worker_queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the worker's QueueWorker drains the notifications queue");

    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "NotifyAcme").await;
    let org_b = create_org(&app, &bootstrap, "NotifyGlobex").await;
    let admin_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let admin_b = format!("Bearer {}", token_for(&org_b, "admin").await);
    let author_id =
        Uuid::parse_str(&create_user(&app, &admin_a, "Author", "author@notify.test").await)
            .expect("valid user uuid");
    let author_a = format!(
        "Bearer {}",
        token_with_sub(&org_a, "admin", author_id).await
    );

    let post_a = create_post(&app, &author_a, "Launch", "Big news").await;

    let notification_count = |bearer: String| {
        let app = &app;
        async move {
            let listed = app
                .http()
                .get("/notifications")
                .header(header::AUTHORIZATION, &bearer)
                .send()
                .await;
            listed.assert_status_is_ok();
            listed.json().await.value().array().iter().count()
        }
    };

    let publish =
        json!({ "query": format!("mutation {{ publishPost(id: \"{post_a}\") {{ id status }} }}") });

    // The first publish succeeds and emits exactly one PostPublishedEvent.
    let first = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&publish)
        .send()
        .await;
    first.assert_status_is_ok();
    assert!(
        first
            .json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_none(),
        "the first publish must succeed",
    );

    // Republishing is now a conflict on GraphQL too — no second event fires, so
    // the worker never receives a duplicate notification.
    let again = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&publish)
        .send()
        .await;
    again.assert_status_is_ok();
    assert!(
        again
            .json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "republishing an already-published post must conflict",
    );

    let mut seen = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if notification_count(admin_a.clone()).await >= 1 {
            seen = true;
            break;
        }
    }
    assert!(
        seen,
        "the worker persisted a notification for org A that GET /notifications returns",
    );

    assert_eq!(
        notification_count(admin_a.clone()).await,
        1,
        "the conflicting republish emitted no second event, so there is exactly one notification",
    );

    assert_eq!(
        notification_count(admin_b).await,
        0,
        "org B must not see org A's notification",
    );

    let a_list = app
        .http()
        .get("/notifications")
        .header(header::AUTHORIZATION, &admin_a)
        .send()
        .await;
    a_list.assert_status_is_ok();
    let a_body = a_list.json().await;
    let messages: Vec<String> = a_body
        .value()
        .array()
        .iter()
        .map(|n| n.object().get("message").string().to_owned())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("Launch")),
        "the persisted notification names the published post: {messages:?}",
    );

    worker_queue
        .shutdown()
        .await
        .expect("the worker's QueueWorker stops cleanly");
}
