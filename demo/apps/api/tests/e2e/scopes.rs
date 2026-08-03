//! OAuth scopes narrowing a token, end to end against the real app and DB.
//!
//! The scenario is the one MCP made ordinary: a user hands a third-party client
//! a token minted for *part* of what they can do. Roles still say who the caller
//! is; the scope says how much of that identity this particular token may
//! exercise. A read-only delegation must therefore read — and be refused, in a
//! way it can act on, the moment it tries to write.
//!
//! The `WWW-Authenticate: … error="insufficient_scope"` challenge that carries
//! that refusal back is asserted in the framework's own suite
//! (`crates/nest-rs-authn/tests/integration/resource/scope.rs`), where the app
//! composes `ProtectedResourceModule`. This app is not the demo's resource
//! server — `assistant` is — so what it proves here is the half that belongs to
//! it: the enforcement.

use features::authz::constants;
use features::testing::{ORG_ID, token_with_scopes};
use nest_rs::testing::TestApp;
use poem::http::{StatusCode, header};
use serde_json::json;
use uuid::Uuid;

use crate::*;

/// A bearer token for `org_id`, subject `sub`, delegated exactly `scopes`.
///
/// The subject is not optional here: `PostAuthorGuard` refuses a subjectless
/// token outright, and a test whose write failed for *that* reason would look
/// like a scope refusal while proving nothing about scopes.
fn narrow_bearer(org_id: &str, sub: Uuid, scopes: &[&str]) -> String {
    format!(
        "Bearer {}",
        token_with_scopes(
            Uuid::parse_str(org_id).expect("valid org uuid"),
            vec![features::Role::Admin],
            Some(sub),
            scopes.iter().map(|s| (*s).to_owned()).collect(),
        )
    )
}

/// An org with one user in it — the author every post write needs.
async fn org_with_author(app: &TestApp, label: &str) -> (String, Uuid) {
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(app, &bootstrap, label).await;
    let admin = format!("Bearer {}", token_for(&org, "admin").await);
    let author = create_user(
        app,
        &admin,
        "Author",
        &format!("author@{}.test", label.to_lowercase()),
    )
    .await;
    let author = Uuid::parse_str(&author).expect("valid user uuid");
    (org, author)
}

async fn create_post_with(app: &TestApp, bearer: &str) -> poem::Response {
    app.http()
        .post("/posts")
        .header(header::AUTHORIZATION, bearer)
        .body_json(&json!({ "title": "Launch", "body": "Big news" }))
        .send()
        .await
        .0
}

#[tokio::test]
async fn a_read_only_delegation_reads_but_cannot_write() {
    let (_db, app) = boot().await;
    let (org, author) = org_with_author(&app, "ScopeAcme").await;

    let read_only = narrow_bearer(&org, author, &[constants::POSTS_READ]);

    // The scope it holds works, all the way through the row filter.
    app.http()
        .get("/posts")
        .header(header::AUTHORIZATION, &read_only)
        .send()
        .await
        .assert_status_is_ok();

    // The one it does not is refused — and this is an *admin* token, so nothing
    // but the scope is standing in the way.
    assert_eq!(
        create_post_with(&app, &read_only).await.status(),
        StatusCode::FORBIDDEN,
        "a read-only delegation must not be able to write",
    );
}

#[tokio::test]
async fn widening_the_delegation_is_what_unblocks_the_write() {
    // The other half of the same claim: the refusal above is about the scope
    // and nothing else, so the identical caller with `posts:write` succeeds.
    // Without this, a bug that refused every write would pass the test above.
    let (_db, app) = boot().await;
    let (org, author) = org_with_author(&app, "ScopeGlobex").await;

    let full = narrow_bearer(
        &org,
        author,
        &[constants::POSTS_READ, constants::POSTS_WRITE],
    );

    let status = create_post_with(&app, &full).await.status();
    assert!(
        status.is_success(),
        "the same caller, one scope wider, may write — got {status}",
    );
}

#[tokio::test]
async fn a_delegation_with_no_post_scope_reads_nothing() {
    // A token scoped to a different capability entirely: the posts rules are
    // withheld, so the read gate refuses rather than returning rows.
    let (_db, app) = boot().await;
    let (org, author) = org_with_author(&app, "ScopeInitech").await;

    let unrelated = narrow_bearer(&org, author, &[constants::AUDIO_TRANSCODE]);

    assert_eq!(
        app.http()
            .get("/posts")
            .header(header::AUTHORIZATION, &unrelated)
            .send()
            .await
            .0
            .status(),
        StatusCode::FORBIDDEN,
    );
}
