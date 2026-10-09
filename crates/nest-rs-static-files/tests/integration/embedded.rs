//! Covers `src/embedded.rs` — a folder compiled into the binary, served with
//! the same rules as a directory, tagged by the digest of its bytes.

use nest_rs_core::module;
use nest_rs_static_files::{Embed, Embedded, StaticFilesModule};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

use crate::{SCRIPT, header, is_problem_404, text};

/// What a use site writes: the derive and its path through this crate, and
/// no `rust-embed` line in the manifest.
#[derive(Embed)]
#[folder = "tests/harness/site"]
#[crate_path = "nest_rs_static_files::rust_embed"]
pub(crate) struct Site;

#[module(imports = [StaticFilesModule::for_root(Embedded::of::<Site>())])]
struct EmbeddedApp;

async fn app() -> TestApp {
    TestApp::for_module::<EmbeddedApp>().await.unwrap()
}

#[tokio::test]
async fn the_folder_is_served_from_the_binary() {
    let app = app().await;

    let script = app.http().get("/assets/app.js").send().await;
    script.assert_status_is_ok();
    script.assert_content_type("text/javascript; charset=utf-8");
    script.assert_header("cache-control", "no-cache");
    assert_eq!(text(script).await, SCRIPT);

    let index = app.http().get("/").send().await;
    index.assert_status_is_ok();
    assert!(text(index).await.contains("<title>Publish</title>"));

    let docs = app.http().get("/docs").send().await;
    docs.assert_status_is_ok();
    assert!(text(docs).await.contains("<title>Publish docs</title>"));
}

#[tokio::test]
async fn a_hidden_file_the_binary_carries_is_never_served() {
    let app = app().await;

    assert!(is_problem_404(app.http().get("/.env").send().await).await);
    app.http()
        .get("/.well-known/security.txt")
        .send()
        .await
        .assert_status_is_ok();
    assert!(is_problem_404(app.http().get("/%2e%2e/Cargo.toml").send().await).await);
}

#[tokio::test]
async fn the_digest_is_the_tag_and_a_range_reads_the_embedded_bytes() {
    let app = app().await;

    let first = app.http().get("/assets/app.js").send().await;
    let etag = header(&first, "etag").unwrap();
    assert_eq!(etag.len(), 66, "a quoted SHA-256: {etag}");

    app.http()
        .get("/assets/app.js")
        .header("if-none-match", &etag)
        .send()
        .await
        .assert_status(StatusCode::NOT_MODIFIED);

    let part = app
        .http()
        .get("/assets/app.js")
        .header("range", "bytes=0-6")
        .send()
        .await;
    part.assert_status(StatusCode::PARTIAL_CONTENT);
    assert_eq!(text(part).await, &SCRIPT[..7]);

    app.http()
        .post("/assets/app.js")
        .send()
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);
}
