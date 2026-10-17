//! The test harness through the umbrella's `testing` feature alone, as a
//! headless suite reaches it: the entry that serves no HTTP, and the test
//! authority.

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
