//! The test harness through the umbrella's `testing` feature alone, as a
//! suite reaches it: the entry that serves no HTTP, the in-process client
//! beside `http`, and the test authority.

use nest_rs::core::anyhow;
use nest_rs::testing::{HeadlessApp, TestAuthority};

use crate::module::MacroHygieneModule;

/// Boot the witness module with no transport, as a worker's suite does.
pub async fn hygiene_headless() -> anyhow::Result<HeadlessApp> {
    HeadlessApp::builder()
        .module::<MacroHygieneModule>()
        .build_headless()
        .await
}

/// The authority a TLS test double presents.
pub fn hygiene_authority() -> TestAuthority {
    TestAuthority::new()
}

/// Boot `M` and send it one request in process, as an app's suite does: the
/// client and the answer it reads name no server type.
#[cfg(feature = "http")]
pub async fn hygiene_status<M: nest_rs::core::Module + 'static>(
    path: &str,
) -> anyhow::Result<nest_rs::http::StatusCode> {
    let app = nest_rs::testing::TestApp::for_module::<M>().await?;
    Ok(app.http().get(path).send().await.0.status())
}
