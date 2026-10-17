use nest_rs::oauth::client::{OAuthClient, TARGET};
use nest_rs::testing::LogCapture;
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

#[tokio::test]
async fn the_social_callback_reads_the_transaction_cookie_the_redirect_set() {
    let logs = LogCapture::install();
    let (_db, app) = boot().await;
    let authorize = app.http().get("/social/github/authorize").send().await;
    let set_cookie = authorize
        .0
        .headers()
        .get("set-cookie")
        .expect("the redirect sets the transaction cookie")
        .to_str()
        .expect("an ascii cookie")
        .to_owned();
    assert!(
        set_cookie.starts_with(&format!("{}=", OAuthClient::TRANSACTION_COOKIE)),
        "{set_cookie}"
    );
    let cookie = set_cookie
        .split(';')
        .next()
        .expect("the cookie's name and value");

    app.http()
        .get("/social/github/callback?code=abc&state=forged")
        .header("cookie", format!("theme=dark; {cookie}"))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let rejected = logs.expect_one(TARGET, "OAuth callback rejected");
    assert_eq!(
        rejected.field("reason").as_deref(),
        Some("csrf_state_mismatch"),
        "the transaction was read and verified; only the forged state stopped it"
    );
}
