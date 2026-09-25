//! `Opaque` tells the operator the whole failure: the sentence a wrapper prints
//! and every cause beneath it, whichever error type the handler returned.

use anyhow::Context;
use nest_rs_http::Opaque;
use nest_rs_testing::LogCapture;

#[test]
fn the_operator_line_carries_every_cause_of_an_anyhow_chain() {
    let capture = LogCapture::install();
    let failed: anyhow::Result<()> = Err(anyhow::anyhow!("connection refused by 10.0.0.5:6379"))
        .context("the queue backend failed");

    let _ = failed.opaque();

    let line = capture.expect_one(nest_rs_http::target::HTTP, "request failed");
    let error = line.field("error").unwrap_or_default();
    assert!(error.contains("the queue backend failed"), "{error}");
    assert!(
        error.contains("connection refused by 10.0.0.5:6379"),
        "{error}"
    );
}
