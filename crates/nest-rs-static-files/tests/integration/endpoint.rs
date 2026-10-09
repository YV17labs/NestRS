//! Covers `src/endpoint.rs` — what a request gets: the file and its headers,
//! `HEAD`, the methods refused, the names never served, conditional and
//! range requests, caching, and the single-page app's navigation fallback.

use std::time::Duration;

use nest_rs_static_files::StaticFilesConfig;
use nest_rs_testing::LogCapture;
use poem::http::StatusCode;

use crate::{INDEX, NAVIGATION, SCRIPT, header, is_problem_404, serve, site, text};

#[tokio::test]
async fn a_file_is_served_with_its_type_validators_and_the_edge_s_headers() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let resp = app.http().get("/assets/app.js").send().await;
    resp.assert_status_is_ok();
    resp.assert_content_type("text/javascript; charset=utf-8");
    resp.assert_header("content-length", SCRIPT.len().to_string());
    resp.assert_header("accept-ranges", "bytes");
    resp.assert_header("cache-control", "no-cache");
    resp.assert_header("x-content-type-options", "nosniff");
    resp.assert_header_exist("etag");
    resp.assert_header_exist("last-modified");
    assert_eq!(text(resp).await, SCRIPT);

    let root = app.http().get("/").send().await;
    root.assert_status_is_ok();
    root.assert_content_type("text/html; charset=utf-8");
    assert_eq!(text(root).await, INDEX);
}

#[tokio::test]
async fn head_answers_the_get_s_headers_without_its_bytes() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let resp = app.http().head("/assets/app.js").send().await;
    resp.assert_status_is_ok();
    resp.assert_header("content-length", SCRIPT.len().to_string());
    resp.assert_content_type("text/javascript; charset=utf-8");
    resp.assert_header_exist("etag");
    assert!(text(resp).await.is_empty());
}

#[tokio::test]
async fn another_method_is_refused_with_the_ones_allowed() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    for resp in [
        app.http().post("/assets/app.js").send().await,
        app.http().put("/").send().await,
        app.http().delete("/assets/app.js").send().await,
    ] {
        resp.assert_status(StatusCode::METHOD_NOT_ALLOWED);
        resp.assert_header("allow", "GET, HEAD");
    }
    assert!(is_problem_404(app.http().post("/assets/missing.js").send().await).await);
}

#[tokio::test]
async fn a_hidden_name_is_never_served_but_a_leading_well_known() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;
    let logs = LogCapture::install();

    for hidden in ["/.env", "/.git/config", "/%2eenv", "/assets/.well-known/x"] {
        assert!(
            is_problem_404(app.http().get(hidden).send().await).await,
            "{hidden} is a missing file's 404"
        );
    }
    let ordinary = logs.find(
        nest_rs_static_files::TARGET,
        "refused a path naming nothing the static files serve",
    );
    assert!(!ordinary.is_empty());
    assert!(ordinary.iter().all(|event| event.level == "debug"));

    let resp = app.http().get("/.well-known/security.txt").send().await;
    resp.assert_status_is_ok();
    resp.assert_content_type("text/plain; charset=utf-8");
}

#[tokio::test]
async fn a_climb_out_of_the_root_is_refused_and_reported() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;
    let logs = LogCapture::install();

    for attempt in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2e%2e%2fsecret.txt",
        "/assets%2fapp.js",
        "/assets%5capp.js",
        "/app%00.js",
    ] {
        assert!(
            is_problem_404(app.http().get(attempt).send().await).await,
            "{attempt} names nothing served"
        );
    }
    let attempts = logs.find(
        nest_rs_static_files::TARGET,
        "refused a path climbing out of the static files' root",
    );
    assert_eq!(attempts.len(), 7, "{:#?}", logs.events());
    assert!(attempts.iter().all(|event| event.level == "warn"));
    assert_eq!(
        attempts[0].field("reason").as_deref(),
        Some("dot_segment"),
        "the line says why, never what was asked",
    );
}

#[tokio::test]
async fn a_path_that_cannot_be_read_is_a_bad_request() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    for malformed in ["/app%zz.js", "/app%2", "/%FF"] {
        let resp = app.http().get(malformed).send().await;
        resp.assert_status(StatusCode::BAD_REQUEST);
        resp.assert_content_type("application/problem+json");
    }
}

#[tokio::test]
async fn a_current_copy_is_answered_not_modified() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let first = app.http().get("/assets/app.js").send().await;
    let etag = header(&first, "etag").unwrap();
    let last_modified = header(&first, "last-modified").unwrap();

    let by_tag = app
        .http()
        .get("/assets/app.js")
        .header("if-none-match", &etag)
        .send()
        .await;
    by_tag.assert_status(StatusCode::NOT_MODIFIED);
    by_tag.assert_header("etag", &etag);
    by_tag.assert_header("cache-control", "no-cache");
    assert!(text(by_tag).await.is_empty());

    app.http()
        .get("/assets/app.js")
        .header("if-modified-since", &last_modified)
        .send()
        .await
        .assert_status(StatusCode::NOT_MODIFIED);

    app.http()
        .get("/assets/app.js")
        .header("if-none-match", "\"stale\"")
        .send()
        .await
        .assert_status_is_ok();
    app.http()
        .get("/assets/app.js")
        .header("if-match", "\"stale\"")
        .send()
        .await
        .assert_status(StatusCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn a_range_is_answered_partially_or_refused_when_it_reaches_nothing() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;
    let len = SCRIPT.len();

    let part = app
        .http()
        .get("/assets/app.js")
        .header("range", "bytes=0-6")
        .send()
        .await;
    part.assert_status(StatusCode::PARTIAL_CONTENT);
    part.assert_header("content-range", format!("bytes 0-6/{len}"));
    part.assert_header("content-length", "7");
    assert_eq!(text(part).await, &SCRIPT[..7]);

    let tail = app
        .http()
        .get("/assets/app.js")
        .header("range", "bytes=-3")
        .send()
        .await;
    tail.assert_status(StatusCode::PARTIAL_CONTENT);
    assert_eq!(text(tail).await, &SCRIPT[len - 3..]);

    let past = app
        .http()
        .get("/assets/app.js")
        .header("range", format!("bytes={len}-"))
        .send()
        .await;
    past.assert_status(StatusCode::RANGE_NOT_SATISFIABLE);
    past.assert_header("content-range", format!("bytes */{len}"));

    let stale = app
        .http()
        .get("/assets/app.js")
        .header("range", "bytes=0-6")
        .header("if-range", "\"stale\"")
        .send()
        .await;
    stale.assert_status_is_ok();
    assert_eq!(text(stale).await, SCRIPT, "a changed file is sent whole");

    let head = app
        .http()
        .head("/assets/app.js")
        .header("range", "bytes=0-6")
        .send()
        .await;
    head.assert_status_is_ok();
}

#[tokio::test]
async fn a_lifetime_caches_every_file_but_an_html_document() {
    let dir = site();
    let app = serve(
        dir.path(),
        StaticFilesConfig {
            max_age: Some(Duration::from_secs(31_536_000)),
            ..StaticFilesConfig::default()
        },
    )
    .await;

    app.http()
        .get("/assets/app.js")
        .send()
        .await
        .assert_header("cache-control", "public, max-age=31536000");
    app.http()
        .get("/")
        .send()
        .await
        .assert_header("cache-control", "no-cache");
    app.http()
        .get("/docs")
        .send()
        .await
        .assert_header("cache-control", "no-cache");
}

#[tokio::test]
async fn a_navigation_falls_back_to_the_index_and_an_api_client_gets_a_404() {
    let dir = site();
    let app = serve(
        dir.path(),
        StaticFilesConfig {
            spa_fallback: true,
            ..StaticFilesConfig::default()
        },
    )
    .await;

    let page = app
        .http()
        .get("/settings/profile")
        .header("accept", NAVIGATION)
        .send()
        .await;
    page.assert_status_is_ok();
    page.assert_content_type("text/html; charset=utf-8");
    page.assert_header("cache-control", "no-cache");
    page.assert_header("vary", "Accept, Sec-Fetch-Mode");
    assert_eq!(text(page).await, INDEX);

    let fetched = app
        .http()
        .get("/settings/profile")
        .header("sec-fetch-mode", "navigate")
        .send()
        .await;
    fetched.assert_status_is_ok();

    let api = app
        .http()
        .get("/settings/profile")
        .header("accept", "application/json")
        .send()
        .await;
    assert_eq!(
        header(&api, "vary").as_deref(),
        Some("Accept, Sec-Fetch-Mode")
    );
    assert!(is_problem_404(api).await, "an API client never gets a page");
    assert!(is_problem_404(app.http().get("/settings/profile").send().await).await);
    assert!(
        is_problem_404(
            app.http()
                .get("/assets/missing.js")
                .header("accept", NAVIGATION)
                .send()
                .await
        )
        .await,
        "a missing file is never a page",
    );
    assert!(
        is_problem_404(
            app.http()
                .post("/settings/profile")
                .header("accept", NAVIGATION)
                .send()
                .await
        )
        .await,
    );
}

#[tokio::test]
async fn without_the_fallback_a_navigation_miss_is_a_404() {
    let dir = site();
    let app = serve(dir.path(), StaticFilesConfig::default()).await;

    let resp = app
        .http()
        .get("/settings/profile")
        .header("accept", NAVIGATION)
        .send()
        .await;
    resp.assert_header_is_not_exist("vary");
    assert!(is_problem_404(resp).await);
}

#[tokio::test]
async fn the_files_answer_under_their_mount_path_alone() {
    let dir = site();
    let app = serve(
        dir.path(),
        StaticFilesConfig {
            path: "/static".into(),
            ..StaticFilesConfig::default()
        },
    )
    .await;

    let resp = app.http().get("/static/assets/app.js").send().await;
    resp.assert_status_is_ok();
    assert_eq!(text(resp).await, SCRIPT);
    app.http().get("/static").send().await.assert_status_is_ok();
    assert!(is_problem_404(app.http().get("/assets/app.js").send().await).await);
    assert!(is_problem_404(app.http().get("/static//assets/app.js").send().await).await);
}
