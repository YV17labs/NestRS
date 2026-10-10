//! Rendering an error as the message an operator reads: its own sentence and
//! every cause beneath it, each said once.

use std::any::Any;
use std::error::Error;

use tokio::task::JoinError;

use crate::error::{DecodeError, DecodeFailures};

const NO_MESSAGE: &str = "an error with no message";

/// `error`'s message, followed by each cause beneath it that the sentence so
/// far does not already end with; a blank message is left out, and a chain
/// saying nothing at all is said to be one.
///
/// **A decode failure anywhere in the chain is said without its value**, as its
/// [`DecodeError`], and a joined task's panic without its payload; every other
/// link is passed through [`DecodeError::redact`]. A wrapper wording the value
/// its own way (`{0:?}`, a `format!` of the field) is not caught.
pub fn error_message(error: &(dyn Error + 'static)) -> String {
    let failures = DecodeFailures::of(Some(error));
    let said = |link: &(dyn Error + 'static)| {
        if let Some(joined) = link.downcast_ref::<JoinError>() {
            return joined_task(joined);
        }
        match DecodeError::of(link) {
            Some(report) => report.to_string(),
            None => failures.redact(&link.to_string()).into_owned(),
        }
    };
    let mut sentence = said(error);
    if sentence.trim().is_empty() {
        sentence.clear();
    }
    let mut source = error.source();
    while let Some(cause) = source {
        let text = said(cause);
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

/// How a joined task ended, as tokio says it of a panic whose payload is no
/// string: the panic was filed, redacted, where it was raised.
fn joined_task(error: &JoinError) -> String {
    if error.is_panic() {
        format!("task {} panicked", error.id())
    } else {
        error.to_string()
    }
}

/// `error` boxed as the framework carries a developer's failure — every link of
/// its chain kept as the type it is, so [`error_message`] reads each one.
///
/// anyhow's own `.into()` boxes a private wrapper whose `source()` skips the
/// error it holds, hiding a decode failure from [`error_message`]; an
/// `anyhow::Error` is unwrapped instead, dropping its backtrace. Every other
/// error converts as `.into()` would.
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
    // Unreachable fallback: `slot` is emptied only by the `take` that returned above.
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

    /// `redis`'s io errors display as their source does.
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

    #[test]
    fn a_cause_that_is_merely_a_substring_of_its_parent_is_kept() {
        let error = Wrapped {
            message: "the queue backend failed",
            cause: Some(io("failed")),
        };
        assert_eq!(error_message(&error), "the queue backend failed: failed");
    }

    #[tokio::test]
    async fn a_joined_task_s_panic_is_said_without_its_payload() {
        let joined =
            tokio::spawn(async { serde_json::from_str::<u64>(r#""sk_live_secret""#).unwrap() })
                .await
                .expect_err("a secret is not a number");
        let task = joined.id();
        assert!(
            joined.to_string().contains("sk_live"),
            "tokio's own sentence"
        );

        let wrapped = Wrapped {
            message: "the writer stopped",
            cause: Some(Box::new(joined)),
        };
        assert_eq!(
            error_message(&wrapped),
            format!("the writer stopped: task {task} panicked")
        );

        let cancelled = tokio::spawn(std::future::pending::<()>());
        cancelled.abort();
        let cancelled = cancelled.await.expect_err("it was aborted");
        assert_eq!(error_message(&cancelled), cancelled.to_string());
    }

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

        let hidden: Box<dyn Error + Send + Sync> = anyhow::Error::from(secret()).into();
        assert!(hidden.downcast_ref::<serde_json::Error>().is_none());
        assert_eq!(error_message(&*hidden), REPORT);
    }

    #[test]
    fn boxed_error_converts_any_other_error_as_into_does() {
        let message = boxed_error("the queue backend failed");
        assert_eq!(message.to_string(), "the queue backend failed");
        let io = boxed_error(std::io::Error::other("timed out"));
        assert!(io.downcast_ref::<std::io::Error>().is_some());
    }

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
