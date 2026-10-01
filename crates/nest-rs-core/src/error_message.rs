//! Rendering an error as the message an operator reads: its own sentence and
//! every cause beneath it.
//!
//! A wrapper's `Display` often names no cause (`the queue backend failed`), so a
//! line carrying it alone says nothing; a cause's `Display` sometimes inlines its
//! own source (`pool timed out: timed out`), so a line appending every source
//! says one thing twice. Two crates had written the walk, one with the second
//! defect and one without; one home keeps the rendering uniform.
//!
//! The walk can only read the links a chain exposes, so the box an edge carries
//! a handler's error in is built here too ([`boxed_error`]): the conversion every
//! edge reached for first hid the error an `anyhow::Error` held.

use std::any::Any;
use std::error::Error;

use crate::error::{DecodeError, DecodeFailures};

/// What a chain saying nothing at all is said as.
const NO_MESSAGE: &str = "an error with no message";

/// `error`'s message, followed by each cause beneath it that the sentence so
/// far does not already end with — a cause a parent inlined as `…: <cause>` is
/// said once, and a message saying nothing, blank included, is left out rather
/// than joined. A chain saying nothing at all is said to be one rather than filed
/// as an empty field: its `Debug` would say as little — `""` for a string error —
/// or more than a log should, a field its `Display` left out.
///
/// **A decode failure anywhere in the chain is said without its value.** A link
/// that is one — serde_json's error, or serde's value error — is rendered as its
/// [`DecodeError`], naming where and what kind: serde quotes the value it
/// refused, and the value is a payload's, which the line an operator reads is no
/// place for. Every other link is said as [`DecodeError::redact`] says its
/// sentence, so a wrapper that spells its cause — `#[error("…: {0}")]`,
/// `#[error(transparent)]`, anyhow's own box — spells the report rather than the
/// value. Where a wrapper words the value its own way (`{0:?}`, a `format!` of
/// the field), nothing in the chain says so, and the sentence is its author's.
pub fn error_message(error: &(dyn Error + 'static)) -> String {
    let failures = DecodeFailures::of(Some(error));
    let said = |link: &(dyn Error + 'static)| match DecodeError::of(link) {
        Some(report) => report.to_string(),
        None => failures.redact(&link.to_string()).into_owned(),
    };
    let mut sentence = said(error);
    if sentence.trim().is_empty() {
        sentence.clear();
    }
    let mut source = error.source();
    while let Some(cause) = source {
        let text = said(cause);
        // `strip_suffix` rather than `ends_with(&format!(": {text}"))`: the
        // formatted needle was a whole `String` built and dropped per cause, for
        // a suffix test — a third of this function's cost, paid at every site
        // that renders an error.
        let inlined = sentence
            .strip_suffix(text.as_str())
            .is_some_and(|before| before.ends_with(": "));
        let said = text.trim().is_empty() || sentence == text || inlined;
        if !said {
            if !sentence.is_empty() {
                sentence.push_str(": ");
            }
            sentence.push_str(&text);
        }
        source = cause.source();
    }
    if sentence.is_empty() {
        return NO_MESSAGE.to_owned();
    }
    sentence
}

/// `error` boxed as the framework carries a developer's failure — every link of
/// its chain kept as the type it is, so [`error_message`] reads each one.
///
/// `Into<Box<dyn Error + Send + Sync>>` is the bound an edge takes a handler's
/// error by, and an `anyhow::Error` meets it through anyhow's own conversion,
/// which boxes anyhow's private wrapper rather than the error inside it: the box
/// displays that error, and its `source()` skips it. A decode failure there is no
/// link of the chain at all, so neither a `downcast` nor [`error_message`] can
/// find it — the documented handler shape, `anyhow::Result` and `?`, was exactly
/// the one whose value reached the line. An `anyhow::Error` is therefore moved out
/// of its wrapper instead (anyhow's
/// `reallocate_into_boxed_dyn_error_without_backtrace`): the outermost error is
/// the box, a `.context(…)` stays a link above its cause, and anyhow's captured
/// backtrace — which no framework line renders — is dropped.
///
/// Every other error converts as it would through `.into()`. `'static` because a
/// type is told apart at run time, by [`Any`].
pub fn boxed_error<E>(error: E) -> Box<dyn Error + Send + Sync>
where
    E: Into<Box<dyn Error + Send + Sync>> + 'static,
{
    let mut slot = Some(error);
    if let Some(anyhow) = (&mut slot as &mut dyn Any)
        .downcast_mut::<Option<anyhow::Error>>()
        .and_then(Option::take)
    {
        return anyhow.reallocate_into_boxed_dyn_error_without_backtrace();
    }
    // `slot` is emptied only by the `take` above, which returned: this is `Some`
    // for every error but an `anyhow::Error`, and the fallback is never reached.
    slot.map_or_else(|| Box::from(NO_MESSAGE), Into::into)
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Debug)]
    struct Wrapped {
        message: &'static str,
        cause: Option<Box<dyn Error + Send + Sync>>,
    }

    impl fmt::Display for Wrapped {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.message)
        }
    }

    impl Error for Wrapped {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.cause
                .as_deref()
                .map(|cause| cause as &(dyn Error + 'static))
        }
    }

    fn io(message: &str) -> Box<dyn Error + Send + Sync> {
        Box::new(std::io::Error::other(message.to_owned()))
    }

    #[test]
    fn every_cause_is_appended_once() {
        let error = Wrapped {
            message: "the queue backend failed",
            cause: Some(Box::new(Wrapped {
                message: "pool timed out",
                cause: Some(io("timed out")),
            })),
        };
        assert_eq!(
            error_message(&error),
            "the queue backend failed: pool timed out: timed out"
        );
    }

    /// A parent that inlined its cause as `…: <cause>`, and one whose text *is*
    /// its cause's — `redis`'s io errors display as their source does.
    #[test]
    fn a_cause_a_parent_already_inlined_is_not_said_twice() {
        let inlined = Wrapped {
            message: "pool timed out: timed out",
            cause: Some(io("timed out")),
        };
        assert_eq!(error_message(&inlined), "pool timed out: timed out");
        let same = Wrapped {
            message: "Connection refused (os error 111)",
            cause: Some(io("Connection refused (os error 111)")),
        };
        assert_eq!(error_message(&same), "Connection refused (os error 111)");
    }

    /// A message saying nothing is no link of the sentence: no leading, trailing
    /// or doubled separator stands for it.
    #[test]
    fn an_empty_message_is_left_out_rather_than_joined() {
        let empty_top = Wrapped {
            message: "",
            cause: Some(io("boom")),
        };
        assert_eq!(error_message(&empty_top), "boom");
        let empty_middle = Wrapped {
            message: "the queue backend failed",
            cause: Some(Box::new(Wrapped {
                message: "",
                cause: Some(io("timed out")),
            })),
        };
        assert_eq!(
            error_message(&empty_middle),
            "the queue backend failed: timed out"
        );
        let empty_last = Wrapped {
            message: "the queue backend failed",
            cause: Some(io("")),
        };
        assert_eq!(error_message(&empty_last), "the queue backend failed");
    }

    /// Blank is nothing too, and a chain saying nothing at all says so rather than
    /// filing an empty field.
    #[test]
    fn a_blank_message_is_nothing_and_a_silent_chain_says_so() {
        let blank_top = Wrapped {
            message: "  ",
            cause: Some(io("boom")),
        };
        assert_eq!(error_message(&blank_top), "boom");
        let silent = Wrapped {
            message: "",
            cause: Some(io(" ")),
        };
        assert_eq!(error_message(&silent), "an error with no message");
    }

    /// Only the inlined shape is skipped: a cause whose text merely appears
    /// inside its parent's is still a cause.
    #[test]
    fn a_cause_that_is_merely_a_substring_of_its_parent_is_kept() {
        let error = Wrapped {
            message: "the queue backend failed",
            cause: Some(io("failed")),
        };
        assert_eq!(error_message(&error), "the queue backend failed: failed");
    }

    /// A decode failure in the chain — the error or a cause — is said without
    /// the value serde quoted, whichever site renders it.
    #[test]
    fn a_decode_failure_in_the_chain_is_said_without_its_value() {
        let decode = || -> Box<dyn Error + Send + Sync> {
            Box::new(serde_json::from_str::<u64>(r#""sk_live_secret""#).expect_err("no number"))
        };
        assert_eq!(
            error_message(&*decode()),
            "invalid type: a string, expected u64 at line 1 column 16"
        );
        let wrapped = Wrapped {
            message: "the queue value could not be converted",
            cause: Some(decode()),
        };
        let said = error_message(&wrapped);
        assert_eq!(
            said,
            "the queue value could not be converted: invalid type: a string, expected u64 at \
             line 1 column 16"
        );
    }

    fn secret() -> serde_json::Error {
        serde_json::from_str::<u64>(r#""sk_live_51HsecretTOKEN""#).expect_err("not a number")
    }

    const REPORT: &str = "invalid type: a string, expected u64 at line 1 column 24";

    /// The documented handler shape — `anyhow::Result` and `?` — through the box
    /// every edge carries a handler's error in. anyhow's own conversion hid the
    /// serde error behind a wrapper that displays it and skips it as a source,
    /// so the line carried the value; `.context(…)` stays a link above it.
    #[test]
    fn a_decode_failure_inside_an_anyhow_error_is_said_without_its_value() {
        let bare = boxed_error(anyhow::Error::from(secret()));
        assert!(
            bare.downcast_ref::<serde_json::Error>().is_some(),
            "the box holds the error anyhow held, not anyhow's wrapper",
        );
        assert_eq!(error_message(&*bare), REPORT);

        let context = boxed_error(anyhow::Error::from(secret()).context("upstream reply"));
        assert_eq!(
            error_message(&*context),
            format!("upstream reply: {REPORT}")
        );

        // anyhow's own box, built without `boxed_error`: the error is no link of
        // the chain, and serde's wording is what is left to read.
        let hidden: Box<dyn Error + Send + Sync> = anyhow::Error::from(secret()).into();
        assert!(hidden.downcast_ref::<serde_json::Error>().is_none());
        assert_eq!(error_message(&*hidden), REPORT);
    }

    /// Every other error converts as `.into()` would.
    #[test]
    fn boxed_error_converts_any_other_error_as_into_does() {
        let message = boxed_error("the queue backend failed");
        assert_eq!(message.to_string(), "the queue backend failed");
        let io = boxed_error(std::io::Error::other("timed out"));
        assert!(io.downcast_ref::<std::io::Error>().is_some());
    }

    /// A wrapper spelling its cause spells the report: one inlining it with
    /// `{0}`, one transparent — whose `source()` skips the decode failure, so only
    /// serde's wording finds it — and one returning its cause as a source while
    /// displaying it whole, which is what the queue's `JobError` does.
    #[test]
    fn a_wrapper_spelling_its_decode_failure_spells_the_report() {
        #[derive(Debug, thiserror::Error)]
        enum Feature {
            #[error("could not read the profile: {0}")]
            Inlined(#[source] serde_json::Error),
            #[error(transparent)]
            Transparent(serde_json::Error),
            #[error("{0:?}")]
            Debugged(serde_json::Error),
        }
        assert_eq!(
            error_message(&Feature::Inlined(secret())),
            format!("could not read the profile: {REPORT}")
        );
        assert_eq!(error_message(&Feature::Transparent(secret())), REPORT);
        let debugged = error_message(&Feature::Debugged(secret()));
        assert!(!debugged.contains("sk_live"), "{debugged}");

        let whole = Wrapped {
            message: "",
            cause: Some(Box::new(secret())),
        };
        assert_eq!(error_message(&whole), REPORT);
        let displays_its_cause = Displays(Box::new(secret()));
        assert_eq!(error_message(&displays_its_cause), REPORT);
    }

    /// A type's `custom` message has no shape to recognise; a wrapper inlining it
    /// is redacted by the chain, which knows the text.
    #[test]
    fn a_custom_decode_message_a_wrapper_inlines_is_said_as_the_report() {
        #[derive(Debug, thiserror::Error)]
        #[error("could not read the token: {0}")]
        struct Token(#[source] serde_json::Error);

        let custom = <serde_json::Error as serde::de::Error>::custom("bad token sk_live_51H");
        assert_eq!(
            error_message(&Token(custom)),
            "could not read the token: a value its type does not accept"
        );
    }

    /// Displays its cause whole and returns it as its source — the queue's
    /// `JobError`.
    #[derive(Debug)]
    struct Displays(Box<dyn Error + Send + Sync>);

    impl fmt::Display for Displays {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            fmt::Display::fmt(&self.0, f)
        }
    }

    impl Error for Displays {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&*self.0)
        }
    }
}
