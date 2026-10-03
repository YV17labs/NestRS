//! Covers `src/error_message.rs`: the line an operator reads for a chain holding
//! a `validator` failure. A service's error, or a developer's own, keeps
//! `ValidationErrors` as its source or spells it, and validator's `Display`
//! prints every rule's parameters, the submitted value among them.

use nest_rs_core::error_message;
use validator::{Validate, ValidationErrors};

/// A live secret, submitted where a password belongs.
const SECRET: &str = "sk_live_51HsecretTOKEN";

/// Two fields refusing the secret, so validator's sentence runs to two lines.
#[derive(Validate)]
struct Signup {
    #[validate(length(min = 32))]
    password: String,
    #[validate(email)]
    email: String,
}

/// `ServiceError::Validation`'s shape: a constant sentence, the failure kept as
/// its source.
#[derive(Debug, thiserror::Error)]
#[error("validation failed")]
struct Refused(#[from] ValidationErrors);

/// A developer's own error inlining the failure it keeps as its source.
#[derive(Debug, thiserror::Error)]
#[error("the signup was refused: {0}")]
struct Inlined(#[from] ValidationErrors);

/// A developer's own error saying the failure as its own.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
struct Transparent(#[from] ValidationErrors);

/// The first eight-character run of `secret` that `text` spells, so a line
/// quoting it cut short or elided still counts as quoting it.
fn quoted_run<'s>(text: &str, secret: &'s str) -> Option<&'s str> {
    (0..=secret.len().saturating_sub(8))
        .filter_map(|start| secret.get(start..start + 8))
        .find(|run| text.contains(run))
}

fn refusal() -> ValidationErrors {
    Signup {
        password: SECRET.to_owned(),
        email: SECRET.to_owned(),
    }
    .validate()
    .unwrap_err()
}

/// A validation failure anywhere in the chain is said without the value it
/// refused, whichever shape holds it, and the line still names the field and
/// the rule it broke.
#[test]
fn a_validation_failure_in_the_chain_is_said_without_the_submitted_value() {
    let said = [
        (
            "kept as the source",
            error_message(&Refused::from(refusal())),
        ),
        ("inlined", error_message(&Inlined::from(refusal()))),
        ("transparent", error_message(&Transparent::from(refusal()))),
    ];

    let quoting: Vec<String> = said
        .iter()
        .filter_map(|(shape, line)| {
            quoted_run(line, SECRET).map(|run| format!("{shape} quotes `{run}`: {line}"))
        })
        .collect();
    assert!(
        quoting.is_empty(),
        "lines quoting the submitted value: {quoting:#?}"
    );
    for (shape, line) in &said {
        assert!(
            line.contains("password") && line.contains("length"),
            "{shape}: the line names the field and the rule: {line}",
        );
    }
}
