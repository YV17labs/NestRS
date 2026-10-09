//! The static files as a client meets them, booted in process through
//! `TestApp`: one module per file of `src/` it exercises.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod disk;
mod embedded;
mod endpoint;
mod module;

use std::path::Path;

use nest_rs_core::Module;
use nest_rs_static_files::{StaticFilesConfig, StaticFilesModule};
use nest_rs_testing::{TestApp, TestAppBuilder};
use poem::test::TestResponse;
use tempfile::TempDir;

/// The tree both sources serve, as the working directory sees it.
pub(crate) const SITE: &str = "tests/harness/site";

/// The site's script, read back byte for byte.
pub(crate) const SCRIPT: &str = include_str!("../harness/site/assets/app.js");

/// The single-page app's entry point.
pub(crate) const INDEX: &str = include_str!("../harness/site/index.html");

/// What a browser sends when it navigates.
pub(crate) const NAVIGATION: &str = "text/html,application/xhtml+xml,*/*;q=0.8";

/// The site, copied where a test may change it, with what git cannot hold: a
/// `.git/` directory, and an empty one.
pub(crate) fn site() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy(Path::new(SITE), dir.path());
    write(dir.path(), ".git/config", "[core]\n");
    std::fs::create_dir_all(dir.path().join("empty")).unwrap();
    dir
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        match entry.file_type().unwrap().is_dir() {
            true => copy(&entry.path(), &target),
            false => {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
}

pub(crate) fn write(root: &Path, name: &str, content: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// `M` booted with the files of `root` under `config`'s other fields, the
/// config seeded as a hermetic test does.
pub(crate) fn files_in<M: Module + 'static>(
    root: &Path,
    config: StaticFilesConfig,
) -> TestAppBuilder {
    TestApp::builder().module::<M>().provide(StaticFilesConfig {
        root: Some(root.to_owned()),
        ..config
    })
}

/// [`files_in`] booted.
pub(crate) async fn serve_in<M: Module + 'static>(
    root: &Path,
    config: StaticFilesConfig,
) -> TestApp {
    files_in::<M>(root, config).build().await.unwrap()
}

/// `root` served alone.
pub(crate) async fn serve(root: &Path, config: StaticFilesConfig) -> TestApp {
    serve_in::<StaticFilesModule>(root, config).await
}

/// A response's header, as text.
pub(crate) fn header(resp: &TestResponse, name: &str) -> Option<String> {
    resp.0
        .headers()
        .get(name)
        .map(|value| value.to_str().unwrap().to_owned())
}

/// A response's body, as text.
pub(crate) async fn text(resp: TestResponse) -> String {
    resp.0.into_body().into_string().await.unwrap()
}

/// Whether `resp` is the framework's `404` problem document, which an API
/// client parses — never a page.
pub(crate) async fn is_problem_404(resp: TestResponse) -> bool {
    resp.0.status() == poem::http::StatusCode::NOT_FOUND
        && header(&resp, "content-type").as_deref() == Some("application/problem+json")
        && !text(resp).await.contains('<')
}
