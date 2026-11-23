//! `#[resilient]`'s pins, read at compile time: a copy of the ranges
//! `nest_rs_resilience::ResilienceConfig` holds its `timeout` and `retries` to,
//! and of every other field of it, the settings its policy holds for every
//! caller, each pinned against the config by a test in `nest-rs-resilience`.

use syn::{Attribute, Expr, ExprLit, Lit};

use crate::args::{site, takes_value};
use crate::job::TIMEOUT;
use crate::ungrouped::ungrouped_expr;

/// The decorator, as it is written.
pub const RESILIENT: &str = "resilient";

/// The key a method pins its retries with.
pub const RETRIES: &str = "retries";

/// Whether `attr` is a `#[resilient]`, however its path is written: read by
/// its last segment, since a macro resolves no name.
pub fn is_resilient(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == RESILIENT)
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
