//! `src/security_headers.rs` reaching the wire, and the one way a value gets
//! there without having been checked.
//!
//! Every value that arrives through configuration is boot-validated
//! (`validate_header_value`, HTTP-S4), so a misconfigured
//! `NESTRS_HTTP__SECURITY_HEADERS__FRAME_OPTIONS` fails the boot naming the
//! variable. `HttpTransport::security_headers` is the other door — a builder
//! call, taking the struct directly, skipping config resolution entirely — and
//! it is what the harness in this file uses.
//!
//! What must not happen there is a security header quietly disappearing. The
//! response is otherwise identical, the app answers `200`, and `X-Frame-Options`
//! is simply absent — which is a clickjacking exposure that no test of the
//! *response* would think to look for, because nothing asked for it to be gone.

use nest_rs_core::{Module, module};
use nest_rs_http::{HttpTransport, SecurityHeadersConfig, controller, routes};
use poem::endpoint::BoxEndpoint;
use poem::test::TestClient;

#[controller(path = "/pages")]
struct PagesController;

#[routes]
impl PagesController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "a page worth framing"
    }
}

#[module(providers = [PagesController])]
struct PagesModule;

/// [`crate::boot`], with a `SecurityHeadersConfig` pinned on the transport
/// rather than resolved from the environment.
async fn boot_with<M>(
    headers: SecurityHeadersConfig,
) -> TestClient<BoxEndpoint<'static, poem::Response>>
where
    M: Module + 'static,
{
    crate::boot_on::<M>(HttpTransport::new().security_headers(headers)).await
}

#[tokio::test]
async fn the_default_headers_are_stamped_on_every_response() {
    // The baseline the failure below is a departure from — without it, a run
    // where *no* security header is ever emitted would satisfy the assertions
    // that follow.
    let client = boot_with::<PagesModule>(SecurityHeadersConfig::default()).await;
    let resp = client.get("/pages").send().await;
    resp.assert_status_is_ok();
    resp.assert_header("x-frame-options", "DENY");
    resp.assert_header("x-content-type-options", "nosniff");
}

#[tokio::test]
async fn a_header_value_that_cannot_be_built_is_reported_rather_than_dropped() {
    let logs = nest_rs_testing::LogCapture::install();
    let client = boot_with::<PagesModule>(SecurityHeadersConfig {
        // A newline: legal in a `String`, refused by `HeaderValue::from_str`,
        // and the shape a value assembled from a template would take.
        frame_options: Some("DENY\nX-Injected: yes".to_owned()),
        ..SecurityHeadersConfig::default()
    })
    .await;

    let resp = client.get("/pages").send().await;
    resp.assert_status_is_ok();
    assert!(
        resp.0.headers().get("x-frame-options").is_none(),
        "a value that cannot be built is not sent — the alternative is \
         smuggling a second header into the response",
    );
    resp.assert_header("x-content-type-options", "nosniff");

    let event = logs.expect_one(
        "nest_rs::http",
        "failed to construct a security header despite boot validation",
    );
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field("header").as_deref(),
        Some("x-frame-options"),
        "the event names which header went missing — the response cannot, \
         since its whole symptom is an absence: {:?}",
        event.fields,
    );
}
