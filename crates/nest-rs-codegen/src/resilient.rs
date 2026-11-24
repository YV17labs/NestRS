//! `#[resilient]`'s pins, read at compile time: a copy of the ranges
//! `nest_rs_resilience::ResilienceConfig` holds its `timeout` and `retries` to,
//! and of every other field of it, the settings its policy holds for every
//! caller, each pinned against the config by a test in `nest-rs-resilience` —
//! and how an impl half generating a method of its own reads the decorator:
//! where it is kept, and the mark the generated method carries.

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::quote;
use syn::{Attribute, Expr, ExprLit, Item, Lit, Meta, Stmt};

use crate::args::{site, takes_value};
use crate::job::{RETRIES, TIMEOUT};
use crate::ungrouped::{ungrouped_expr, ungrouped_tokens};

/// The decorator, as it is written.
pub const RESILIENT: &str = "resilient";

/// Whether `attr` is a `#[resilient]`, however its path is written: read by
/// its last segment, since a macro resolves no name.
pub fn is_resilient(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == RESILIENT)
}

/// One attribute of a developer's method, parted between that method and the
/// method an impl half generates beside it.
pub struct ResilientParted {
    /// What the developer's method keeps: the decorator, under the conditions
    /// it was written under.
    pub kept: Option<Attribute>,
    /// What the generated method is handed: everything else.
    pub handed: Option<Attribute>,
}

/// `attr` parted by [`is_resilient`]'s reading: a `#[resilient]` is kept, and a
/// `#[cfg_attr]` is split by entry — each `resilient(…)` it holds, a nested
/// `cfg_attr`'s included, kept under the same predicates, every other entry
/// handed under them. Any other attribute, and a `#[cfg_attr]` holding no
/// `resilient(…)`, is handed as written.
pub fn resilient_parted(attr: &Attribute) -> ResilientParted {
    if is_resilient(attr) {
        return ResilientParted {
            kept: Some(attr.clone()),
            handed: None,
        };
    }
    let entries = match &attr.meta {
        Meta::List(list) if attr.path().is_ident("cfg_attr") => {
            parted_cfg_attr(list.tokens.clone())
        }
        _ => None,
    };
    let Some((kept, handed)) = entries else {
        return ResilientParted {
            kept: None,
            handed: Some(attr.clone()),
        };
    };
    let holding = |tokens: TokenStream| {
        let mut attr = attr.clone();
        if let Meta::List(list) = &mut attr.meta {
            list.tokens = tokens;
        }
        attr
    };
    ResilientParted {
        kept: Some(holding(kept)),
        handed: handed.map(holding),
    }
}

/// The arguments of one `cfg_attr(predicate, ..)` as two, the first holding its
/// `resilient(…)` entries and the second, when any is left, the others — or
/// `None` when it holds no `resilient(…)`, or is not the attribute's grammar.
fn parted_cfg_attr(arguments: TokenStream) -> Option<(TokenStream, Option<TokenStream>)> {
    let mut entries = crate::attrs::split_at_commas(arguments).into_iter();
    let predicate = entries.next().filter(|predicate| !predicate.is_empty())?;
    let (mut kept, mut handed) = (Vec::new(), Vec::new());
    for entry in entries {
        let entry = ungrouped_tokens(entry);
        if entry.is_empty() {
            continue;
        }
        let mut tokens = entry.clone().into_iter();
        let nested = match (tokens.next(), tokens.next(), tokens.next()) {
            (Some(TokenTree::Ident(name)), Some(TokenTree::Group(group)), None)
                if name == "cfg_attr" && group.delimiter() == Delimiter::Parenthesis =>
            {
                parted_cfg_attr(group.stream())
            }
            _ => None,
        };
        match nested {
            Some((nested_kept, nested_handed)) => {
                kept.push(quote!(cfg_attr(#nested_kept)));
                handed.extend(nested_handed.map(|handed| quote!(cfg_attr(#handed))));
            }
            None if entry_is_resilient(&entry) => kept.push(entry),
            None => handed.push(entry),
        }
    }
    if kept.is_empty() {
        return None;
    }
    let handed = (!handed.is_empty()).then(|| quote!(#predicate, #(#handed),*));
    Some((quote!(#predicate, #(#kept),*), handed))
}

/// Whether a `cfg_attr`'s entry is a `resilient(…)`, read as [`is_resilient`]
/// reads an attribute: by the last segment of the path it opens on.
fn entry_is_resilient(entry: &TokenStream) -> bool {
    let mut last = None;
    for token in entry.clone() {
        match token {
            TokenTree::Ident(segment) => last = Some(segment),
            TokenTree::Punct(punct) if punct.as_char() == ':' => {}
            _ => break,
        }
    }
    last.is_some_and(|segment| segment == RESILIENT)
}

/// The item a field `#[operations]` generates opens its body on, in the macro
/// ABI's prefix: no developer writes it, so a decorator finding it knows it was
/// handed to that field rather than to the method the developer wrote.
const OPERATION_FIELD: &str = "__nestrs_operation_field";

/// The mark a field `#[operations]` generates opens its body on: an item rustc
/// reports as no dead code, its name leading with `_`, so it carries no lint
/// level a crate's `forbid` could refuse.
pub fn operation_field_mark() -> TokenStream {
    let mark = syn::Ident::new(OPERATION_FIELD, proc_macro2::Span::call_site());
    quote!(fn #mark() {})
}

/// Whether `stmts`, a method's body, opens on [`operation_field_mark`].
pub fn opens_on_operation_field_mark(stmts: &[Stmt]) -> bool {
    matches!(
        stmts.first(),
        Some(Stmt::Item(Item::Fn(mark))) if mark.sig.ident == OPERATION_FIELD
    )
}

/// A mechanism the policy runs for every caller of its dependency.
#[derive(Clone, Copy)]
enum Mechanism {
    Backoff,
    Breaker,
    Bulkhead,
    Rate,
}

impl Mechanism {
    /// What it does for every caller: why no method holds one of its own.
    const fn shared(self) -> &'static str {
        match self {
            Self::Backoff => "the backoff spaces the retries of every caller of the policy",
            Self::Breaker => {
                "the circuit breaker counts the failures of every caller of the policy"
            }
            Self::Bulkhead => {
                "the bulkhead bounds the calls every caller of the policy runs at once"
            }
            Self::Rate => "the rate limit counts the attempts every caller of the policy sends",
        }
    }
}

/// A setting of `ResilienceConfig` the policy holds for every caller, which a
/// method pins none of.
struct PolicySetting {
    /// The field, and the key a method would write.
    field: &'static str,
    /// The key its variable is read under, a duration's unit after it.
    var: &'static str,
    mechanism: Mechanism,
}

/// Every field of `ResilienceConfig` but the two a method pins, `timeout` and
/// `retries`, in the config's order.
const POLICY_SETTINGS: [PolicySetting; 13] = [
    PolicySetting {
        field: "backoff_first",
        var: "BACKOFF_FIRST_MS",
        mechanism: Mechanism::Backoff,
    },
    PolicySetting {
        field: "backoff_ceiling",
        var: "BACKOFF_CEILING_MS",
        mechanism: Mechanism::Backoff,
    },
    PolicySetting {
        field: "breaker_window",
        var: "BREAKER_WINDOW",
        mechanism: Mechanism::Breaker,
    },
    PolicySetting {
        field: "breaker_failures",
        var: "BREAKER_FAILURES",
        mechanism: Mechanism::Breaker,
    },
    PolicySetting {
        field: "breaker_open",
        var: "BREAKER_OPEN_SECS",
        mechanism: Mechanism::Breaker,
    },
    PolicySetting {
        field: "breaker_age",
        var: "BREAKER_AGE_SECS",
        mechanism: Mechanism::Breaker,
    },
    PolicySetting {
        field: "breaker_probes",
        var: "BREAKER_PROBES",
        mechanism: Mechanism::Breaker,
    },
    PolicySetting {
        field: "concurrency",
        var: "CONCURRENCY",
        mechanism: Mechanism::Bulkhead,
    },
    PolicySetting {
        field: "concurrency_wait",
        var: "CONCURRENCY_WAIT_MS",
        mechanism: Mechanism::Bulkhead,
    },
    PolicySetting {
        field: "concurrency_waiters",
        var: "CONCURRENCY_WAITERS",
        mechanism: Mechanism::Bulkhead,
    },
    PolicySetting {
        field: "rate_limit",
        var: "RATE_LIMIT",
        mechanism: Mechanism::Rate,
    },
    PolicySetting {
        field: "rate_window",
        var: "RATE_WINDOW_SECS",
        mechanism: Mechanism::Rate,
    },
    PolicySetting {
        field: "rate_wait",
        var: "RATE_WAIT_MS",
        mechanism: Mechanism::Rate,
    },
];

/// The refusal of `key` when it names a setting the policy holds for every
/// caller — a key `#[resilient]` does not take, and no misspelling: it says
/// which mechanism shares it, and the variable and the field that set it.
pub fn resilient_policy_setting(key: &str) -> Option<String> {
    let setting = POLICY_SETTINGS
        .iter()
        .find(|setting| setting.field == key)?;
    Some(format!(
        "#[{RESILIENT}] takes no `{key}`: {} — set it on the policy: `{}`, or \
         `ResilienceConfig::{key}` in code",
        setting.mechanism.shared(),
        setting.var,
    ))
}

/// One end of a range and why nothing past it works — the runtime's reason,
/// word for word.
struct End {
    count: u64,
    why: &'static str,
}

const TIMEOUT_FLOOR_MILLIS: End = End {
    count: 1,
    why: "a zero timeout gives every attempt up before its call is sent, and one above zero but \
          under a millisecond ends at the next tick of tokio's timer, which counts whole \
          milliseconds and cannot honour less",
};

const TIMEOUT_CEILING_MILLIS: End = End {
    count: 10 * 60 * 1000,
    why: "one attempt past ten minutes outlasts a worker job's default deadline, the longest the \
          framework gives a unit; a call that long is a job's work, started and then polled, \
          rather than one attempt",
};

const RETRIES_CEILING: End = End {
    count: 10,
    why: "past ten, the retries let one call send its failing dependency twelve attempts or more, \
          the hammering a breaker exists to stop",
};

/// Read a `timeout = "…"` a `#[resilient]` method pins, in milliseconds: a
/// duration literal from one millisecond to ten minutes, the range
/// `ResilienceConfig::timeout` is held to.
pub fn resilient_timeout(value: &Expr) -> syn::Result<u64> {
    let millis = crate::duration_millis(RESILIENT, Some(TIMEOUT), value)?;
    let refused = |must: &str, end: &End| {
        let unit = if end.count == 1 {
            "millisecond"
        } else {
            "milliseconds"
        };
        syn::Error::new_spanned(
            ungrouped_expr(value),
            format!(
                "{}: must be {must} {} {unit} — {}",
                site(RESILIENT, Some(TIMEOUT)),
                end.count,
                end.why,
            ),
        )
    };
    if millis < TIMEOUT_FLOOR_MILLIS.count {
        return Err(refused("at least", &TIMEOUT_FLOOR_MILLIS));
    }
    if millis > TIMEOUT_CEILING_MILLIS.count {
        return Err(refused("at most", &TIMEOUT_CEILING_MILLIS));
    }
    Ok(millis)
}

/// Read a `retries = N` a `#[resilient]` method pins: a whole number from 1 to
/// 10, the range `ResilienceConfig::retries` is held to. `0`, which turns the
/// policy's retries off, is refused saying a method attempted once is no
/// idempotent one.
pub fn resilient_retries(value: &Expr) -> syn::Result<u32> {
    let value = ungrouped_expr(value);
    let Expr::Lit(ExprLit {
        lit: Lit::Int(int), ..
    }) = value
    else {
        return Err(syn::Error::new_spanned(
            value,
            takes_value(
                RESILIENT,
                Some(RETRIES),
                "a whole number, e.g. `retries = 3`",
            ),
        ));
    };
    // A count past what `u64` holds is past every ceiling, as the runtime reads it.
    let count = int.base10_parse::<u64>().unwrap_or(u64::MAX);
    if count == 0 {
        return Err(syn::Error::new_spanned(
            int,
            format!(
                "{}: `0` turns retries off, which leaves `idempotent` sending the call no second \
                 time — drop `idempotent` and `retries`, and the body runs once",
                site(RESILIENT, Some(RETRIES)),
            ),
        ));
    }
    match u32::try_from(count) {
        Ok(retries) if count <= RETRIES_CEILING.count => Ok(retries),
        _ => Err(syn::Error::new_spanned(
            int,
            format!(
                "{}: must be at most {} — {}",
                site(RESILIENT, Some(RETRIES)),
                RETRIES_CEILING.count,
                RETRIES_CEILING.why,
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn refused(result: syn::Result<impl std::fmt::Debug>) -> String {
        result.expect_err("refused").to_string()
    }

    #[test]
    fn the_decorator_is_read_by_its_last_segment_however_its_path_is_written() {
        let read = |attr: Attribute| is_resilient(&attr);
        assert!(read(parse_quote!(#[resilient(courier)])));
        assert!(read(
            parse_quote!(#[nest_rs::resilience::resilient(courier, idempotent)])
        ));
        assert!(read(
            parse_quote!(#[::nest_rs::prelude::resilient(courier)])
        ));
        assert!(!read(parse_quote!(#[tracing::instrument(skip(self))])));
        assert!(!read(parse_quote!(#[resilient::other(courier)])));
    }

    fn parted(attr: Attribute) -> (Option<String>, Option<String>) {
        let ResilientParted { kept, handed } = resilient_parted(&attr);
        let written = |attr: Attribute| quote!(#attr).to_string();
        (kept.map(written), handed.map(written))
    }

    #[test]
    fn the_decorator_is_kept_and_any_other_attribute_handed_as_written() {
        assert_eq!(
            parted(parse_quote!(#[resilient(courier, idempotent)])),
            (
                Some(quote!(#[resilient(courier, idempotent)]).to_string()),
                None
            )
        );
        for other in [
            parse_quote!(#[tracing::instrument(skip_all)]),
            parse_quote!(#[cfg_attr(test, allow(unused), traced(skip(resilient)))]),
            parse_quote!(#[cfg_attr(test, resilient::other(courier))]),
            parse_quote!(#[cfg_attr(resilient, must_use)]),
            parse_quote!(#[cfg_attr]),
            parse_quote!(#[cfg_attr()]),
        ] {
            let written = quote!(#other).to_string();
            assert_eq!(parted(other), (None, Some(written)));
        }
    }

    /// Each entry of a `cfg_attr` goes where it would have gone written plain,
    /// under the predicate it was written under and every one around it.
    #[test]
    fn a_cfg_attr_is_split_by_entry_under_the_predicates_it_was_written_under() {
        assert_eq!(
            parted(parse_quote!(
                #[cfg_attr(feature = "shielded", resilient(courier, timeout = "2s"), traced(skip_all))]
            )),
            (
                Some(
                    quote!(#[cfg_attr(feature = "shielded", resilient(courier, timeout = "2s"))])
                        .to_string()
                ),
                Some(quote!(#[cfg_attr(feature = "shielded", traced(skip_all))]).to_string()),
            )
        );
        assert_eq!(
            parted(parse_quote!(#[cfg_attr(all(), nest_rs::resilience::resilient(courier),)])),
            (
                Some(
                    quote!(#[cfg_attr(all(), nest_rs::resilience::resilient(courier))]).to_string()
                ),
                None,
            )
        );
        assert_eq!(
            parted(parse_quote!(
                #[cfg_attr(a, cfg(unix), cfg_attr(b, expect(dead_code), resilient(courier)), cfg_attr(c, must_use))]
            )),
            (
                Some(quote!(#[cfg_attr(a, cfg_attr(b, resilient(courier)))]).to_string()),
                Some(
                    quote!(
                        #[cfg_attr(a, cfg(unix), cfg_attr(b, expect(dead_code)), cfg_attr(c, must_use))]
                    )
                    .to_string()
                ),
            )
        );
    }

    #[test]
    fn a_decorator_passed_through_a_macro_fragment_is_kept() {
        let fragment = proc_macro2::Group::new(Delimiter::None, quote!(resilient(courier)));
        let split = parted_cfg_attr(quote!(all(), #fragment, must_use))
            .map(|(kept, handed)| (kept.to_string(), handed.map(|handed| handed.to_string())));
        assert_eq!(
            split,
            Some((
                quote!(all(), resilient(courier)).to_string(),
                Some(quote!(all(), must_use).to_string()),
            ))
        );
    }

    /// The mark is read where `#[operations]` writes it, the first thing a
    /// generated field's body holds, and nowhere a developer's body could
    /// carry the name by another road.
    #[test]
    fn the_operation_fields_mark_is_read_where_it_opens_a_body_alone() {
        let mark = operation_field_mark();
        let field: syn::Block = parse_quote!({
            #mark
            run().await
        });
        assert!(opens_on_operation_field_mark(&field.stmts));
        for written in [
            parse_quote!({ run().await }),
            parse_quote!({
                let __nestrs_operation_field = ();
                run().await
            }),
            parse_quote!({
                fn operation_field() {}
                run().await
            }),
            parse_quote!({
                run().await;
                #mark
            }),
            parse_quote!({}),
        ] {
            let written: syn::Block = written;
            assert!(!opens_on_operation_field_mark(&written.stmts));
        }
    }

    #[test]
    fn a_timeout_is_read_in_milliseconds_up_to_ten_minutes() {
        assert_eq!(resilient_timeout(&parse_quote!("2s")).ok(), Some(2_000));
        assert_eq!(resilient_timeout(&parse_quote!("1ms")).ok(), Some(1));
        assert_eq!(resilient_timeout(&parse_quote!("10m")).ok(), Some(600_000));
        let past = refused(resilient_timeout(&parse_quote!("601s")));
        assert_eq!(
            past,
            format!(
                "#[resilient] `timeout`: must be at most 600000 milliseconds — {}",
                TIMEOUT_CEILING_MILLIS.why
            )
        );
    }

    #[test]
    fn a_timeout_that_is_no_duration_literal_is_refused_by_the_grammar() {
        for value in [
            parse_quote!(2000),
            parse_quote!("0s"),
            parse_quote!(TIMEOUT),
        ] {
            assert!(
                refused(resilient_timeout(&value))
                    .starts_with("#[resilient] `timeout` takes a duration literal"),
            );
        }
    }

    #[test]
    fn retries_are_read_from_one_to_ten() {
        assert_eq!(resilient_retries(&parse_quote!(1)).ok(), Some(1));
        assert_eq!(resilient_retries(&parse_quote!(10)).ok(), Some(10));
        for past in [parse_quote!(11), parse_quote!(99999999999999999999999)] {
            assert_eq!(
                refused(resilient_retries(&past)),
                format!(
                    "#[resilient] `retries`: must be at most 10 — {}",
                    RETRIES_CEILING.why
                )
            );
        }
    }

    #[test]
    fn zero_retries_say_to_drop_idempotent() {
        assert!(refused(resilient_retries(&parse_quote!(0))).contains("drop `idempotent`"));
    }

    #[test]
    fn a_setting_of_the_policys_is_refused_naming_its_mechanism_its_variable_and_its_field() {
        assert_eq!(
            resilient_policy_setting("rate_window").as_deref(),
            Some(
                "#[resilient] takes no `rate_window`: the rate limit counts the attempts every \
                 caller of the policy sends — set it on the policy: `RATE_WINDOW_SECS`, or \
                 `ResilienceConfig::rate_window` in code"
            )
        );
        for setting in &POLICY_SETTINGS {
            let refused = resilient_policy_setting(setting.field).expect("a policy's setting");
            assert!(
                refused.contains(setting.mechanism.shared())
                    && refused.contains(&format!("`{}`", setting.var))
                    && refused.contains(&format!("`ResilienceConfig::{}`", setting.field)),
                "{refused}"
            );
        }
    }

    /// A pin the decorator takes and a misspelling are no setting of the
    /// policy's: the grammar answers them.
    #[test]
    fn a_pin_and_a_misspelling_name_no_setting_of_the_policys() {
        for key in [TIMEOUT, RETRIES, "idempotent", "rate", "rate_limits"] {
            assert_eq!(resilient_policy_setting(key), None, "{key}");
        }
    }

    /// A mechanism's settings share its word first, so a row filed under
    /// another mechanism would tell a method that mechanism's reason.
    #[test]
    fn each_setting_is_the_mechanism_its_field_names_first() {
        for setting in &POLICY_SETTINGS {
            let word = match setting.mechanism {
                Mechanism::Backoff => "backoff",
                Mechanism::Breaker => "breaker",
                Mechanism::Bulkhead => "concurrency",
                Mechanism::Rate => "rate",
            };
            assert_eq!(
                setting.field.split('_').next(),
                Some(word),
                "`{}`",
                setting.field
            );
        }
    }

    #[test]
    fn retries_that_are_no_whole_number_are_refused_naming_what_they_take() {
        for value in [parse_quote!("3"), parse_quote!(-1), parse_quote!(1.5)] {
            assert_eq!(
                refused(resilient_retries(&value)),
                "#[resilient] `retries` takes a whole number, e.g. `retries = 3`"
            );
        }
    }
}
