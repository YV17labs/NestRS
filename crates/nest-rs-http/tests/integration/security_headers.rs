//! Security headers on the wire, through `HttpTransport::security_headers`,
//! the builder door that skips config validation: an invalid value must not
//! make a header quietly disappear.

use nest_rs_core::{Module, module};
use nest_rs_http::{HttpSecurityHeaders, HttpTransport, controller, routes};
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

/// [`crate::boot`], with a `HttpSecurityHeaders` pinned on the transport
/// rather than resolved from the environment.
async fn boot_with<M>(
    headers: HttpSecurityHeaders,
) -> TestClient<BoxEndpoint<'static, poem::Response>>
where
    M: Module + 'static,
{
    crate::boot_on::<M>(HttpTransport::new().security_headers(headers)).await
}

#[tokio::test]
async fn the_default_headers_are_stamped_on_every_response() {
    // The baseline: without it, a run emitting no header would pass below.
    let client = boot_with::<PagesModule>(HttpSecurityHeaders::default()).await;
    let resp = client.get("/pages").send().await;
    resp.assert_status_is_ok();
    resp.assert_header("x-frame-options", "DENY");
    resp.assert_header("x-content-type-options", "nosniff");
}

#[tokio::test]
async fn a_header_value_that_cannot_be_built_is_reported_rather_than_dropped() {
    let logs = nest_rs_testing::LogCapture::install();
    let client = boot_with::<PagesModule>(HttpSecurityHeaders {
        // A newline: legal in a `String`, refused by `HeaderValue::from_str`.
        frame_options: Some("DENY\nX-Injected: yes".to_owned()),
        ..HttpSecurityHeaders::default()
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
