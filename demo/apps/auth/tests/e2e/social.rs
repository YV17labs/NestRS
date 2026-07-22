use poem::http::StatusCode;

use super::harness::*;

#[tokio::test]
async fn the_social_authorize_endpoint_redirects_to_the_provider() {
    let (_db, app) = boot().await;
    let resp = app.http().get("/social/github/authorize").send().await;
    resp.assert_status(StatusCode::FOUND);
    resp.assert_header_exist("location");
    resp.assert_header_exist("set-cookie");
    let location = resp.0.headers().get("location").expect("location header");
    assert!(
        location
            .to_str()
            .expect("ascii location")
            .starts_with("https://github.com/login/oauth/authorize"),
        "redirect must hit GitHub, got {location:?}",
    );
}

#[tokio::test]
async fn the_provider_path_segment_is_case_insensitive() {
    let (_db, app) = boot().await;
    let resp = app.http().get("/social/GitHub/authorize").send().await;
    resp.assert_status(StatusCode::FOUND);
    let location = resp.0.headers().get("location").expect("location header");
    assert!(
        location
            .to_str()
            .expect("ascii location")
            .starts_with("https://github.com/login/oauth/authorize"),
        "case-insensitive provider must still hit GitHub, got {location:?}",
    );
}

#[tokio::test]
async fn a_configured_provider_that_is_not_imported_is_unknown() {
    let (_db, app) = boot().await;
    app.http()
        .get("/social/gitlab/authorize")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_social_callback_rejects_a_forged_state() {
    let (_db, app) = boot().await;
    app.http()
        .get("/social/github/callback?code=abc&state=forged")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
