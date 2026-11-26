//! Rendering an error as the message an operator reads: its own sentence and
//! every cause beneath it, each said once, and a driver's error as its driver
//! says it ([`SafeLink`]).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::error::Error;
use std::marker::PhantomData;
use std::sync::Arc;

use ref_cast::RefCastCustom;
use ref_cast::ref_cast_custom;
use tokio::task::JoinError;

use self::__private::link_as;
use crate::error::{DecodeError, Redactions};

const NO_MESSAGE: &str = "an error with no message";

/// `error`'s message, followed by each cause beneath it that the sentence so
/// far does not already end with; a blank message is left out, and a chain
/// saying nothing at all is said to be one.
///
/// **A decode failure anywhere in the chain is said without its value**, as its
/// [`DecodeError`], a joined task's panic without its payload, and **a link a
/// driver claims through a [`SafeLink`] as the driver says it**, with nothing
/// below it; every other link is passed through [`DecodeError::redact`], which
/// also says a claimed link wherever a wrapper spells it whole. A link that is
/// a pointer `std` makes an error of — what an error naming its source behind
/// one hands out — is read as the error it holds, by every reader: an `Arc` or
/// a `Box` of the type read, and an `Arc<dyn Error>` holding any of them. A
/// wrapper wording the value its own way (`{0:?}`, a `format!` of the field,
/// part of a driver's message) is not caught.
pub fn error_message(error: &(dyn Error + 'static)) -> String {
    let redactions = Redactions::of(Some(error));
    let mut sentence = String::new();
    let mut link = Some(error);
    while let Some(current) = link {
        let (text, whole) = said(current, &redactions);
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
        link = if whole { None } else { current.source() };
    }
    if sentence.is_empty() {
        return NO_MESSAGE.to_owned();
    }
    sentence
}

/// What one link of a chain says, and whether that is the whole of what is
/// left — a claimed link's driver says its causes too.
fn said(link: &(dyn Error + 'static), redactions: &Redactions) -> (String, bool) {
    if let Some(joined) = link_as::<JoinError>(link) {
        return (joined_task(joined), false);
    }
    if let Some(sentence) = SafeLink::said(link) {
        return (sentence, true);
    }
    let text = match DecodeError::of(link) {
        Some(report) => report.to_string(),
        None => redactions.redact(&link.to_string()).into_owned(),
    };
    (text, false)
}

/// How a driver says one of its error types without the values it quotes:
/// one entry per type, submitted with `nest_rs_core::inventory::submit!`.
///
/// A database server quotes the input it refused in its message, and a driver's
/// error repeats that message in its own sentence and in its causes, so no
/// renderer reading `Display` can keep it out of a log line, a dead letter or a
/// reply. [`error_message`] and [`DecodeError::redact`] ask the sayer of each
/// link's type before its `Display`: `say` answers `Some(sentence)` — the
/// link's whole meaning, without a value — and the chain is said no further
/// below it; `None` for a value of its type it leaves as it reads.
/// Identifiers — a code, a constraint, a table, a column — are schema, not
/// payload, and stay in the sentence: they are what an operator searches for.
///
/// **The type is read once, here, for every sayer**: where the link is the
/// error itself, where it is the pointer `std` makes an error of — an `Arc` or
/// a `Box` of it, which an error naming its source behind one hands out, and
/// an `Arc<dyn Error>` holding any of those, `Send` and `Sync` or not — and
/// where it is an I/O error holding one, which displays the error it holds
/// whole. A sayer names its type in its signature and downcasts nothing.
///
/// **A type has one sayer.** Two functions claiming one type refuse the boot,
/// naming the type and both ([`ContestedSafeLinkError`]): which of two would
/// speak is link order, and the other's sentence would go quiet in every
/// line. An app wording a library's error its own way claims an error type of
/// its own holding that one as its source. One function linked twice — a
/// crate's unit tests link the crate again through a dev-dependency on it — is
/// one sayer. A binary building no [`App`](crate::App) runs no such check, and
/// reads the first entry linked.
///
/// ```
/// use nest_rs_core::{SafeLink, error_message};
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("row refused: {value}")]
/// struct Refused {
///     code: &'static str,
///     value: String,
/// }
///
/// fn say(refused: &Refused) -> Option<String> {
///     Some(format!("row refused {}", refused.code))
/// }
///
/// nest_rs_core::inventory::submit! { SafeLink::of(&say) }
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("the import failed")]
/// struct Import(#[source] std::sync::Arc<Refused>);
///
/// # fn main() {
/// let refused = Refused { code: "23505", value: "ada@example.com".into() };
/// assert_eq!(
///     error_message(&Import(refused.into())),
///     "the import failed: row refused 23505",
/// );
/// # }
/// ```
///
/// [`ContestedSafeLinkError`]: crate::ContestedSafeLinkError
pub struct SafeLink {
    claim: &'static dyn Claim,
}

impl SafeLink {
    /// The entry saying a `T` through `say`, held by reference: a function's
    /// name is a constant, so `SafeLink::of(&say)` is one too.
    ///
    /// A sayer claims one type, the one its signature names: one taking any
    /// link, to downcast it itself, claims none.
    ///
    /// ```compile_fail,E0277
    /// use std::error::Error;
    ///
    /// use nest_rs_core::SafeLink;
    ///
    /// fn say(link: &(dyn Error + 'static)) -> Option<String> {
    ///     link.is::<std::fmt::Error>().then(|| "formatting failed".to_owned())
    /// }
    ///
    /// nest_rs_core::inventory::submit! { SafeLink::of(&say) }
    /// ```
    pub const fn of<T, F>(say: &'static F) -> Self
    where
        T: Error + 'static,
        F: Fn(&T) -> Option<String> + Sync + 'static,
    {
        Self {
            claim: Typed::<T, F>::of(say),
        }
    }

    /// What the sayer of `link`'s type says of it — `link` itself, or the
    /// error a pointer or an I/O error handed out as the link holds.
    pub(crate) fn said(link: &(dyn Error + 'static)) -> Option<String> {
        let mut link = pointed(link);
        loop {
            if let Some(said) = inventory::iter::<Self>().find_map(|entry| entry.claim.say(link)) {
                return said;
            }
            // Its sentence is the held error's, whole, and its source that
            // error's: the link is the error it holds.
            link = pointed(read_as::<std::io::Error>(link)?.get_ref()?);
        }
    }

    /// The first type two of the linked entries claim, with both sayers.
    pub(crate) fn contested() -> Option<crate::ContestedSafeLinkError> {
        Self::contested_among(inventory::iter::<Self>())
    }

    fn contested_among<'s>(
        entries: impl Iterator<Item = &'s Self>,
    ) -> Option<crate::ContestedSafeLinkError> {
        let mut claimed: HashMap<TypeId, &'static str> = HashMap::new();
        for entry in entries {
            let (id, link) = entry.claim.link();
            let sayer = entry.claim.sayer();
            // One function linked twice is one sayer: a crate's unit tests
            // link the crate again through a dev-dependency on it.
            match claimed.insert(id, sayer) {
                Some(first) if first != sayer => {
                    return Some(crate::ContestedSafeLinkError {
                        link,
                        first,
                        second: sayer,
                    });
                }
                _ => {}
            }
        }
        None
    }
}

inventory::collect!(SafeLink);

/// One sayer, its type erased: what a [`SafeLink`] holds.
trait Claim: Sync {
    /// The type claimed, and its name.
    fn link(&self) -> (TypeId, &'static str);

    /// The function saying it, by its path.
    fn sayer(&self) -> &'static str;

    /// `None` when `link` is no error of the type claimed; else what its
    /// sayer answers.
    fn say(&self, link: &(dyn Error + 'static)) -> Option<Option<String>>;
}

/// A sayer `F` read as the claim of the `T` it takes.
#[derive(RefCastCustom)]
#[repr(transparent)]
struct Typed<T, F>(F, PhantomData<fn(&T)>);

impl<T, F> Typed<T, F> {
    #[ref_cast_custom]
    const fn of(say: &F) -> &Self;
}

impl<T, F> Claim for Typed<T, F>
where
    T: Error + 'static,
    F: Fn(&T) -> Option<String> + Sync + 'static,
{
    fn link(&self) -> (TypeId, &'static str) {
        (TypeId::of::<T>(), std::any::type_name::<T>())
    }

    fn sayer(&self) -> &'static str {
        std::any::type_name::<F>()
    }

    fn say(&self, link: &(dyn Error + 'static)) -> Option<Option<String>> {
        read_as::<T>(link).map(&self.0)
    }
}

/// The error `link` is, or the one an `Arc<dyn Error>` handed out as the link
/// holds, however many hold one another: `std` makes that pointer an error
/// displaying the one it holds and handing out its source, which no downcast
/// reads.
fn pointed<'e>(mut link: &'e (dyn Error + 'static)) -> &'e (dyn Error + 'static) {
    loop {
        link = if let Some(shared) = link.downcast_ref::<Arc<dyn Error + Send + Sync>>() {
            &**shared
        } else if let Some(shared) = link.downcast_ref::<Arc<dyn Error + Send>>() {
            &**shared
        } else if let Some(shared) = link.downcast_ref::<Arc<dyn Error>>() {
            &**shared
        } else {
            return link;
        };
    }
}

/// The `T` that `link` is, or that an `Arc<T>` or a `Box<T>` handed out as the
/// link holds.
fn read_as<'e, T: Error + 'static>(link: &'e (dyn Error + 'static)) -> Option<&'e T> {
    link.downcast_ref::<T>()
        .or_else(|| link.downcast_ref::<Arc<T>>().map(|held| &**held))
        .or_else(|| link.downcast_ref::<Box<T>>().map(|held| &**held))
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

/// `error_message` is private: its tier-2 items live here, reached only through
/// the crate's `__private`.
pub(crate) mod __private {
    use std::error::Error;
    use std::fmt;

    use ref_cast::RefCast;

    /// The `T` that `link` is, or that a pointer handed out as the link holds:
    /// an `Arc<T>` or a `Box<T>`, and an `Arc<dyn Error>` holding any of the
    /// three. How a reader of a chain reads a link whose type no
    /// [`SafeLink`](crate::SafeLink) of its own claims — a sayer the causes
    /// beneath its error: `std` makes each of those pointers an error
    /// displaying the one it holds, so an error naming its source behind one
    /// hands out the pointer, which a downcast to `T` misses.
    pub fn link_as<'e, T: Error + 'static>(link: &'e (dyn Error + 'static)) -> Option<&'e T> {
        super::read_as(super::pointed(link))
    }

    /// `link` as the report a line says it by, without its value, when it is
    /// a decode failure — serde_json's error, or serde's own value error — or
    /// a pointer handed out as the link holds one: how a sayer reading the
    /// chain beneath an error of its own says one.
    pub fn decode_failure(link: &(dyn Error + 'static)) -> Option<crate::DecodeError> {
        crate::DecodeError::of(link)
    }

    /// What the driver claiming `link` says of it, its causes included, and
    /// `None` for a link no [`SafeLink`](crate::SafeLink) claims: how a sayer
    /// reading the chain beneath an error of its own says a link that is
    /// another driver's.
    pub fn claimed_link(link: &(dyn Error + 'static)) -> Option<String> {
        crate::SafeLink::said(link)
    }

    /// A failure as every reader of a chain reads it: its `Display` and its
    /// `Debug` are what [`error_message`](fn@crate::error_message) says of it,
    /// and it hands out nothing below.
    ///
    /// What a holder hands out as its `source()` where it keeps a driver's error,
    /// or a cause, for its own `match`: a `{:?}`, an `anyhow` report and a `main`
    /// print each link's own `Display` and `Debug`, past every
    /// [`SafeLink`](crate::SafeLink), and a driver's quote the input it refused.
    /// A view of the field, so the holder keeps the type it was given; a source
    /// no `match` reads is held as one ([`new`](Self::new)).
    #[derive(RefCast)]
    #[repr(transparent)]
    pub struct SaidFailure<E>(E);

    impl<E: AsRef<dyn Error + Send + Sync>> SaidFailure<E> {
        /// `failure`, read as `error_message` says it.
        pub fn of(failure: &E) -> &Self {
            Self::ref_cast(failure)
        }

        /// `failure`, held to be read as `error_message` says it.
        pub fn new(failure: E) -> Self {
            Self(failure)
        }
    }

    impl<E: AsRef<dyn Error + Send + Sync>> fmt::Display for SaidFailure<E> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&crate::error_message(self.0.as_ref()))
        }
    }

    impl<E: AsRef<dyn Error + Send + Sync>> fmt::Debug for SaidFailure<E> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            fmt::Debug::fmt(&crate::error_message(self.0.as_ref()), f)
        }
    }

    impl<E: AsRef<dyn Error + Send + Sync>> Error for SaidFailure<E> {}

    /// The first `T` in `error`'s chain, `error` itself first, down to an
    /// opaque failure's cause: how an edge finds the framework vocabulary a
    /// failure carries wherever it was wrapped, a link handed out as a pointer
    /// to it included ([`link_as`]).
    pub fn find_in_chain<'e, T: Error + 'static>(
        error: &'e (dyn Error + 'static),
    ) -> Option<&'e T> {
        answerable_links(error).find_map(link_as::<T>)
    }

    /// `error`'s chain, `error` itself first, ending at an
    /// [`OpaqueCause`](crate::__private::OpaqueCause), bare or behind a
    /// pointer: what is beneath one is the operator's to read, never the
    /// client's to be answered.
    pub(crate) fn answerable_links<'e>(
        error: &'e (dyn Error + 'static),
    ) -> impl Iterator<Item = &'e (dyn Error + 'static)> {
        std::iter::successors(Some(error), |&link| {
            if link_as::<crate::__private::OpaqueCause>(link).is_some() {
                None
            } else {
                link.source()
            }
        })
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
    fn the_first_link_of_a_type_is_found_wherever_it_was_wrapped() {
        use super::__private::find_in_chain;

        let error = Wrapped {
            message: "the queue backend failed",
            cause: Some(Box::new(Wrapped {
                message: "pool timed out",
                cause: Some(io("timed out")),
            })),
        };
        let found = find_in_chain::<std::io::Error>(&error).map(ToString::to_string);
        assert_eq!(found.as_deref(), Some("timed out"));
        assert!(find_in_chain::<std::fmt::Error>(&error).is_none());
        assert!(
            find_in_chain::<Wrapped>(&error)
                .is_some_and(|top| top.message.starts_with("the queue"))
        );
    }

    #[test]
    fn what_an_opaque_failure_keeps_is_said_and_never_found() {
        use std::sync::Arc;

        use super::__private::find_in_chain;
        use crate::__private::OpaqueCause;

        let opaque = Wrapped {
            message: "internal error",
            cause: Some(Box::new(OpaqueCause::new(
                "the upstream reply did not decode",
                Some(Arc::new(secret())),
            ))),
        };
        let wrapped = boxed_error(anyhow::Error::new(opaque).context("syncing the ledger"));

        assert_eq!(
            error_message(&*wrapped),
            format!(
                "syncing the ledger: internal error: the upstream reply did not decode: {REPORT}"
            )
        );
        assert!(find_in_chain::<serde_json::Error>(&*wrapped).is_none());
        assert!(
            find_in_chain::<Wrapped>(&*wrapped).is_some(),
            "above it, found"
        );
    }

    /// A link handed out as the pointer an error names its source behind is
    /// read as the error it holds, by the edges' search as by every sayer: the
    /// vocabulary it carries is found, and an opaque failure stops the search.
    #[test]
    fn a_link_behind_a_pointer_is_found_and_an_opaque_one_stops_the_search() {
        use super::__private::find_in_chain;
        use crate::__private::OpaqueCause;

        let shared = Naming(Arc::new(std::io::Error::other("timed out")));
        assert!(
            find_in_chain::<std::io::Error>(&shared).is_some(),
            "behind an Arc"
        );
        let boxed = Naming(Box::new(std::io::Error::other("timed out")));
        assert!(
            find_in_chain::<std::io::Error>(&boxed).is_some(),
            "behind a Box"
        );
        let erased: Arc<dyn Error + Send + Sync> = Arc::new(std::io::Error::other("timed out"));
        assert!(
            find_in_chain::<std::io::Error>(&Naming(erased)).is_some(),
            "behind an erased Arc"
        );

        let cause = || {
            OpaqueCause::new(
                "the upstream reply did not decode",
                Some(Arc::new(secret())),
            )
        };
        assert!(find_in_chain::<serde_json::Error>(&Naming(Arc::new(cause()))).is_none());
        assert!(find_in_chain::<serde_json::Error>(&Naming(Box::new(cause()))).is_none());
        let erased: Arc<dyn Error + Send + Sync> = Arc::new(cause());
        assert!(find_in_chain::<serde_json::Error>(&Naming(erased)).is_none());
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

    /// A driver's error quoting a value in its sentence and again in its cause,
    /// as a database server's message does.
    #[derive(Debug, thiserror::Error)]
    #[error("row rejected: {value}")]
    struct Rejected {
        code: &'static str,
        value: &'static str,
        #[source]
        server: Quoted,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("the server refused {0}")]
    struct Quoted(&'static str);

    fn rejected() -> Rejected {
        Rejected {
            code: "R42",
            value: "\"secret\"",
            server: Quoted("\"secret\""),
        }
    }

    /// The driver's sayer: a code, never the value.
    fn say_rejected(rejected: &Rejected) -> Option<String> {
        Some(format!("row rejected {}", rejected.code))
    }

    inventory::submit! { SafeLink::of(&say_rejected) }

    #[test]
    fn a_link_a_driver_claims_is_said_by_its_sayer_and_nothing_below_it() {
        let said = error_message(&rejected());
        assert_eq!(said, "row rejected R42");

        let wrapped = Wrapped {
            message: "the import failed",
            cause: Some(Box::new(rejected())),
        };
        assert_eq!(
            error_message(&wrapped),
            "the import failed: row rejected R42"
        );
    }

    #[test]
    fn a_wrapper_spelling_a_claimed_link_spells_what_its_driver_says() {
        #[derive(Debug, thiserror::Error)]
        #[error("could not store the row: {0}")]
        struct Inlined(#[source] Rejected);

        assert_eq!(
            error_message(&Inlined(rejected())),
            "could not store the row: row rejected R42"
        );
        assert_eq!(
            error_message(&Displays(Box::new(rejected()))),
            "row rejected R42"
        );
        let hidden: Box<dyn Error + Send + Sync> = anyhow::Error::from(rejected())
            .context("the job failed")
            .into();
        let said = error_message(&*hidden);
        assert!(!said.contains("secret"), "{said}");
        assert!(said.contains("row rejected R42"), "{said}");
    }

    #[test]
    fn a_rendered_text_is_redacted_by_the_claimed_links_of_its_chain() {
        let error = rejected();
        let text = format!("upstream answered: {error}");
        assert_eq!(
            DecodeError::redact(&text, Some(&error)),
            "upstream answered: row rejected R42"
        );
        assert_eq!(DecodeError::redact(&text, None), text, "no chain, no sayer");
    }

    /// A refusal whose sayer goes on from its own sentence, as one telling a
    /// class beside it does.
    #[derive(Debug, thiserror::Error)]
    #[error("the call was refused")]
    struct Refused;

    fn say_refused(_: &Refused) -> Option<String> {
        Some("the call was refused: 503 UNAVAILABLE".to_owned())
    }

    inventory::submit! { SafeLink::of(&say_refused) }

    #[test]
    fn a_text_already_spelling_what_a_sayer_goes_on_with_is_not_said_twice() {
        let said = error_message(&Refused);
        assert_eq!(said, "the call was refused: 503 UNAVAILABLE");
        assert_eq!(
            DecodeError::redact(&said, Some(&Refused)),
            said,
            "a text built from the chain's own message"
        );
        assert_eq!(
            DecodeError::redact(
                "the call was refused, then the call was refused",
                Some(&Refused)
            ),
            "the call was refused: 503 UNAVAILABLE, then the call was refused: 503 UNAVAILABLE",
            "each spelling of the link alone goes on"
        );
    }

    /// What thiserror's `#[source]` makes of a field: a sentence of the
    /// error's own, and the field handed out as the link, a pointer included.
    #[derive(Debug, thiserror::Error)]
    #[error("the import failed")]
    struct Naming<S: Error + 'static>(#[source] S);

    #[test]
    fn a_claimed_link_behind_an_erased_pointer_is_said_by_its_sayer() {
        type Shared = Arc<dyn Error + Send + Sync>;
        let erased: Shared = Arc::new(rejected());
        assert!(erased.to_string().contains("secret"), "std's own sentence");
        assert_eq!(
            error_message(&Naming(Arc::clone(&erased))),
            "the import failed: row rejected R42"
        );

        let twice: Shared = Arc::new(erased);
        assert_eq!(
            error_message(&Naming(twice)),
            "the import failed: row rejected R42"
        );
        let unsendable: Arc<dyn Error> = Arc::new(rejected());
        assert_eq!(
            error_message(&Naming(unsendable)),
            "the import failed: row rejected R42"
        );
        let unshareable: Arc<dyn Error + Send> = Arc::new(rejected());
        assert_eq!(
            error_message(&Naming(unshareable)),
            "the import failed: row rejected R42"
        );

        let text = format!("upstream answered: {}", rejected());
        let erased: Shared = Arc::new(rejected());
        assert_eq!(
            DecodeError::redact(&text, Some(&Naming(erased))),
            "upstream answered: row rejected R42"
        );
    }

    /// The sayer names its type and nothing else: the pointer an error hands
    /// out as its link, and the I/O error a reader over a stream makes of one,
    /// are read here.
    #[test]
    fn a_claimed_type_is_said_behind_every_pointer_handed_out_as_the_link() {
        const SAID: &str = "the import failed: row rejected R42";

        assert_eq!(error_message(&Naming(rejected())), SAID);
        assert_eq!(error_message(&Naming(Arc::new(rejected()))), SAID);
        assert_eq!(error_message(&Naming(Box::new(rejected()))), SAID);
        let boxed: Arc<dyn Error + Send + Sync> = Arc::new(Box::new(rejected()));
        assert_eq!(error_message(&Naming(boxed)), SAID);
        let shared: Arc<dyn Error + Send + Sync> = Arc::new(Arc::new(rejected()));
        assert_eq!(error_message(&Naming(shared)), SAID);

        let read = std::io::Error::other(rejected());
        assert!(read.to_string().contains("secret"), "std's own sentence");
        assert_eq!(error_message(&Naming(read)), SAID);
        let read = std::io::Error::other(Arc::new(rejected()));
        assert_eq!(error_message(&Naming(Arc::new(read))), SAID);

        let text = format!("upstream answered: {}", rejected());
        assert_eq!(
            DecodeError::redact(&text, Some(&Naming(Box::new(rejected())))),
            "upstream answered: row rejected R42"
        );
    }

    #[test]
    fn an_io_error_holding_no_claimed_error_is_said_as_it_reads() {
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(
            error_message(&Naming(refused)),
            "the import failed: connection refused"
        );
        let other = std::io::Error::other(Quoted("nothing"));
        assert_eq!(
            error_message(&Naming(other)),
            "the import failed: the server refused nothing"
        );
    }

    /// A value its sayer leaves is said as it reads, its chain read on.
    #[derive(Debug, thiserror::Error)]
    #[error("lookup failed")]
    struct Declined(#[source] Quoted);

    fn say_declined(_: &Declined) -> Option<String> {
        None
    }

    inventory::submit! { SafeLink::of(&say_declined) }

    #[test]
    fn a_value_its_sayer_leaves_is_said_as_it_reads() {
        assert_eq!(
            error_message(&Declined(Quoted("nothing"))),
            "lookup failed: the server refused nothing"
        );
    }

    #[test]
    fn two_sayers_of_one_type_are_contested_naming_the_type_and_both() {
        fn other(_: &Rejected) -> Option<String> {
            None
        }

        let first = SafeLink::of(&say_rejected);
        let declined = SafeLink::of(&say_declined);
        assert!(SafeLink::contested_among([&first, &declined].into_iter()).is_none());
        let again = SafeLink::of(&say_rejected);
        assert!(
            SafeLink::contested_among([&first, &again].into_iter()).is_none(),
            "one function linked twice is one sayer"
        );

        let second = SafeLink::of(&other);
        let contested = SafeLink::contested_among([&first, &declined, &second].into_iter())
            .expect("two sayers of `Rejected`");
        assert!(contested.link.ends_with("::Rejected"), "{}", contested.link);
        assert!(
            contested.first.ends_with("::say_rejected"),
            "{}",
            contested.first
        );
        assert!(
            contested.second.ends_with("::other"),
            "{}",
            contested.second
        );
        let said = contested.to_string();
        assert!(
            said.contains(contested.first) && said.contains(contested.second),
            "{said}"
        );
    }

    /// The suite's own entries, as every boot reads them.
    #[test]
    fn the_entries_linked_here_claim_a_type_each() {
        assert!(SafeLink::contested().is_none());
    }

    #[test]
    fn a_link_is_read_as_its_type_through_the_pointers_handed_out_as_the_link() {
        use super::__private::link_as;

        let code = |link: &(dyn Error + 'static)| link_as::<Rejected>(link).map(|own| own.code);
        assert_eq!(code(&rejected()), Some("R42"));
        assert_eq!(code(&Arc::new(rejected())), Some("R42"));
        assert_eq!(code(&Box::new(rejected())), Some("R42"));
        let erased: Arc<dyn Error + Send + Sync> = Arc::new(Box::new(rejected()));
        assert_eq!(code(&erased), Some("R42"));
        assert_eq!(code(&Quoted("\"secret\"")), None);
        assert_eq!(code(&Arc::new(Quoted("\"secret\""))), None);
    }

    #[test]
    fn a_link_another_driver_claims_is_said_to_a_sayer_asking_for_it() {
        use super::__private::claimed_link;

        assert_eq!(
            claimed_link(&rejected()).as_deref(),
            Some("row rejected R42")
        );
        assert_eq!(claimed_link(&Quoted("\"secret\"")), None);
    }

    #[tokio::test]
    async fn a_joined_task_s_panic_behind_a_pointer_is_said_without_its_payload() {
        let joined =
            tokio::spawn(async { serde_json::from_str::<u64>(r#""sk_live_secret""#).unwrap() })
                .await
                .expect_err("a secret is not a number");
        let task = joined.id();

        assert_eq!(
            error_message(&Naming(Arc::new(joined))),
            format!("the import failed: task {task} panicked")
        );
    }

    /// serde_json's error is no `Clone`, so an error sharing one holds it
    /// behind an `Arc`: a message a `Deserialize` impl wrote, which no shape
    /// of serde's rebuilds, is read off the type behind the pointer.
    #[test]
    fn a_decode_failure_behind_a_pointer_is_said_without_its_value() {
        let custom = || <serde_json::Error as serde::de::Error>::custom("bad token sk_live_51H");
        let erased: Arc<dyn Error + Send + Sync> = Arc::new(custom());
        for said in [
            error_message(&Naming(custom())),
            error_message(&Naming(Arc::new(custom()))),
            error_message(&Naming(Box::new(custom()))),
            error_message(&Naming(erased)),
        ] {
            assert_eq!(said, "the import failed: a value its type does not accept");
        }
        assert_eq!(
            error_message(&Naming(Arc::new(secret()))),
            format!("the import failed: {REPORT}")
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
