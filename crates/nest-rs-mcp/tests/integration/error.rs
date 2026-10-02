//! Covers `src/error.rs` — what `.opaque()` says about a tool body's own decode
//! failure. The model is told the one constant sentence; the operator's line
//! carries the cause, said without the value it refused, through the documented
//! shape: `anyhow::Result` and `?`.

use nest_rs_core::anyhow::{self, Context};
use nest_rs_core::serde::de::Error as _;
use nest_rs_core::serde::de::value::Error as ValueError;
use nest_rs_mcp::Opaque;
use nest_rs_testing::LogCapture;

const SECRET: &str = "sk_live_51HsecretTOKEN";

/// What the operator's line says of `failed`, after checking the model is told
/// nothing of it.
fn said(failed: anyhow::Result<u64>) -> String {
    let logs = LogCapture::install();
    let refused = failed
        .opaque()
        .expect_err("the failure survives as a failure");
    assert!(
        !format!("{refused:?}").contains(SECRET),
        "the model is told nothing of it: {refused:?}"
    );
    logs.expect_one(nest_rs_mcp::TARGET, "mcp operation failed")
        .field("error")
        .expect("the line carries the cause")
}

/// serde's own quoting sentence, behind `?` and behind a `.context(…)`.
#[test]
fn an_anyhow_chained_decode_failure_is_said_without_its_value() {
    let body = format!("\"{SECRET}\"");
    let bare: anyhow::Result<u64> = serde_json::from_str(&body).map_err(anyhow::Error::from);
    let text = said(bare);
    assert!(!text.contains(SECRET), "{text}");
    assert!(
        text.contains("expected u64"),
        "what failed is still said: {text}"
    );

    let text = said(serde_json::from_str(&body).context("the upstream reply"));
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.starts_with("the upstream reply: "), "{text}");
}

/// A decode failure worded by the type itself — the shape no reading of
/// serde's wording recognises, which only the chain reaches. anyhow's own box
/// hides the error it holds from `source()`, so this is the case that fails
/// when `.opaque()` boxes with `.into()` rather than `boxed_error`.
#[test]
fn a_decode_failure_in_a_type_s_own_words_is_said_without_its_value() {
    let refused = ValueError::custom(format!("token {SECRET} is not ours"));
    let text = said(Err(anyhow::Error::new(refused)));
    assert!(!text.contains(SECRET), "{text}");
}
