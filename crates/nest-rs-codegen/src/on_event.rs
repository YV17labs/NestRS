//! `#[on_event]`'s keys: the worker-job family's `transactional` and
//! `timeout`, read with the family's parsers at a decorator outside its table —
//! a listener is no worker job, and the table's other cells are queue and
//! schedule concepts.

use proc_macro2::TokenStream;
use syn::{Attribute, Meta};

use crate::args::{needs_a_value, site};
use crate::grammar::Grammar;
use crate::job::{
    TIMEOUT, TRANSACTIONAL, job_timeout, job_transaction, timeout_value_at,
    transactional_needs_a_value, transactional_value_at,
};

/// The decorator, as it is written without `#[…]`.
const ON_EVENT: &str = "on_event";

/// What each `transactional` value does at a listener, stated by both of its
/// refusals.
const WHAT_THE_VALUES_DO: &str = "`true` (the default) runs each call of the listener as one \
     transaction of its own, rolled back when it fails; `false` runs it on the pool, where each \
     statement commits on its own";

/// `#[on_event]`'s grammar: a key it does not take is refused listing the two
/// it does.
pub const ON_EVENT_GRAMMAR: Grammar =
    Grammar::new(ON_EVENT, &[TRANSACTIONAL, TIMEOUT]).bare(on_event_needs_a_value);

/// The refusal of a key written bare: the `transactional` one states what each
/// value does.
fn on_event_needs_a_value(attr: &str, key: &str) -> String {
    if key == TRANSACTIONAL {
        transactional_needs_a_value(attr, WHAT_THE_VALUES_DO)
    } else {
        needs_a_value(attr, key)
    }
}

/// The keys one `#[on_event]` declares, read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OnEventArgs {
    /// `transactional`, `None` when unwritten.
    pub transactional: Option<bool>,
    /// `timeout`, in milliseconds, `None` when unwritten.
    pub timeout: Option<u64>,
}

impl OnEventArgs {
    /// The `TransactionPolicy` the listener's unit runs under, rooted at
    /// `surface`, the path re-exporting `nest_rs_database` the calling macro
    /// emits through.
    pub fn transaction(&self, surface: &TokenStream) -> TokenStream {
        job_transaction(self.transactional, surface)
    }

    /// The listener's deadline: the kernel's `JOB_TIMEOUT` unless written.
    pub fn timeout(&self) -> TokenStream {
        job_timeout(self.timeout)
    }
}

/// Read one `#[on_event]` — bare, or with its keys in a list.
pub fn parse_on_event_args(attr: &Attribute) -> syn::Result<OnEventArgs> {
    let mut args = OnEventArgs::default();
    match &attr.meta {
        Meta::Path(_) => {}
        Meta::List(_) => ON_EVENT_GRAMMAR.parse_attr(attr, |arg| {
            match arg.key() {
                TRANSACTIONAL => {
                    args.transactional = Some(transactional_value_at(
                        ON_EVENT,
                        &arg.expr()?,
                        WHAT_THE_VALUES_DO,
                    )?);
                }
                TIMEOUT => args.timeout = Some(timeout_value_at(ON_EVENT, &arg.expr()?)?),
                unread => {
                    return Err(syn::Error::new_spanned(
                        arg.ident(),
                        format!(
                            "{}: the grammar gives this decorator the key and its parser does not \
                             read it — a defect in the framework, not in this code",
                            site(ON_EVENT, Some(unread)),
                        ),
                    ));
                }
            }
            Ok(())
        })?,
        Meta::NameValue(_) => {
            return Err(syn::Error::new_spanned(
                attr,
                "#[on_event] takes its keys in a list — write `#[on_event(timeout = \"30s\")]`",
            ));
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    fn read(attr: Attribute) -> Result<OnEventArgs, String> {
        parse_on_event_args(&attr).map_err(|refusal| refusal.to_string())
    }

    #[test]
    fn a_bare_listener_takes_the_defaults() {
        assert_eq!(
            read(syn::parse_quote!(#[on_event])),
            Ok(OnEventArgs::default())
        );
    }

    #[test]
    fn both_keys_are_read_alone_and_together() {
        assert_eq!(
            read(syn::parse_quote!(#[on_event(transactional = false)])),
            Ok(OnEventArgs {
                transactional: Some(false),
                timeout: None,
            }),
        );
        assert_eq!(
            read(syn::parse_quote!(#[on_event(timeout = "30s")])),
            Ok(OnEventArgs {
                transactional: None,
                timeout: Some(30_000),
            }),
        );
        assert_eq!(
            read(syn::parse_quote!(#[on_event(timeout = "2m", transactional = true)])),
            Ok(OnEventArgs {
                transactional: Some(true),
                timeout: Some(120_000),
            }),
        );
    }

    #[test]
    fn a_job_key_is_unknown_here_and_the_refusal_lists_the_two_keys() {
        assert_eq!(
            read(syn::parse_quote!(#[on_event(retries = 1)])),
            Err(
                "unknown #[on_event] argument `retries`; expected `transactional` or `timeout`"
                    .to_owned()
            ),
        );
    }

    #[test]
    fn a_timeout_keeps_the_job_familys_range_named_at_this_site() {
        let zero = read(syn::parse_quote!(#[on_event(timeout = "0s")])).expect_err("zero");
        assert!(
            zero.starts_with(
                "#[on_event] `timeout` takes a duration literal: a whole number above zero"
            ),
            "{zero}"
        );
        let past = read(syn::parse_quote!(#[on_event(timeout = "25h")])).expect_err("past a day");
        assert!(
            past.starts_with("#[on_event] `timeout` is at most `\"24h\"`"),
            "{past}"
        );
        assert_eq!(
            read(syn::parse_quote!(#[on_event(timeout = "24h")])).map(|args| args.timeout),
            Ok(Some(86_400_000)),
        );
    }

    #[test]
    fn a_transactional_refusal_says_what_each_value_does_at_a_listener() {
        let bare = read(syn::parse_quote!(#[on_event(transactional)])).expect_err("bare");
        assert!(
            bare.starts_with("#[on_event] `transactional` needs a value")
                && bare.contains(WHAT_THE_VALUES_DO),
            "{bare}"
        );
        let wrong =
            read(syn::parse_quote!(#[on_event(transactional = "no")])).expect_err("a string");
        assert!(
            wrong.starts_with("#[on_event] `transactional` takes `true` or `false`")
                && wrong.contains(WHAT_THE_VALUES_DO),
            "{wrong}"
        );
    }

    #[test]
    fn keys_written_outside_a_list_are_refused_with_the_list_form() {
        assert_eq!(
            read(syn::parse_quote!(#[on_event = "30s"])),
            Err(
                "#[on_event] takes its keys in a list — write `#[on_event(timeout = \"30s\")]`"
                    .to_owned()
            ),
        );
    }

    #[test]
    fn with_no_key_the_registration_carries_the_kernels_job_timeout_and_a_unit_of_its_own() {
        let args = read(syn::parse_quote!(#[on_event])).expect("bare");
        assert_eq!(
            args.timeout().to_string(),
            quote!(::nest_rs_core::JOB_TIMEOUT).to_string(),
        );
        let surface = quote!(::nest_rs_events::__private);
        assert_eq!(
            args.transaction(&surface).to_string(),
            quote!(::nest_rs_events::__private::nest_rs_database::TransactionPolicy::PerUnit)
                .to_string(),
        );
        let pool = read(syn::parse_quote!(#[on_event(transactional = false, timeout = "1s")]))
            .expect("both keys");
        assert_eq!(
            pool.timeout().to_string(),
            quote!(::core::time::Duration::from_millis(1000u64)).to_string(),
        );
        assert_eq!(
            pool.transaction(&surface).to_string(),
            quote!(::nest_rs_events::__private::nest_rs_database::TransactionPolicy::Pool)
                .to_string(),
        );
    }
}
