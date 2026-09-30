//! Rendering an error as the message an operator reads: its own sentence and
//! every cause beneath it.
//!
//! A wrapper's `Display` often names no cause (`the queue backend failed`), so a
//! line carrying it alone says nothing; a cause's `Display` sometimes inlines its
//! own source (`pool timed out: timed out`), so a line appending every source
//! says one thing twice. Two crates had written the walk, one with the second
//! defect and one without; one home keeps the rendering uniform.

use std::error::Error;

/// `error`'s message, followed by each cause beneath it that the sentence so
/// far does not already end with — a cause a parent inlined as `…: <cause>` is
/// said once, and a message saying nothing, blank included, is left out rather
/// than joined. A chain saying nothing at all is said to be one rather than filed
/// as an empty field: its `Debug` would say as little — `""` for a string error —
/// or more than a log should, a field its `Display` left out.
///
/// **A decode failure anywhere in the chain is said without its value.** A
/// [`serde_json::Error`] — the error itself or any cause beneath it — is rendered
/// as its [`DecodeError`](crate::DecodeError), naming where and what kind: serde
/// quotes the value it refused, and the value is a payload's, which the line an
/// operator reads is no place for. A wrapper that spells its cause into its own
/// sentence is its author's to word.
pub fn error_message(error: &(dyn Error + 'static)) -> String {
    const NO_MESSAGE: &str = "an error with no message";

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

/// One link of the chain, as it is said: its own sentence, or a decode
/// failure's without the value.
fn said(error: &(dyn Error + 'static)) -> String {
    match error.downcast_ref::<serde_json::Error>() {
        Some(decode) => crate::DecodeError::new(decode).to_string(),
        None => error.to_string(),
    }
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
}
