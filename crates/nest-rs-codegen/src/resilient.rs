//! `#[resilient]`'s pins, read at compile time: a copy of the ranges
//! `nest_rs_resilience::ResilienceConfig` holds its `timeout` and `retries` to,
//! pinned against them by a test in `nest-rs-resilience`.

use syn::{Expr, ExprLit, Lit};

use crate::args::{site, takes_value};
use crate::job::TIMEOUT;
use crate::ungrouped::ungrouped_expr;

/// The decorator, as it is written.
pub const RESILIENT: &str = "resilient";

/// The key a method pins its retries with.
pub const RETRIES: &str = "retries";

/// One end of a range and why nothing past it works — the runtime's reason,
/// word for word.
struct End {
    count: u64,
    why: &'static str,
}

const TIMEOUT_FLOOR_MILLIS: End = End {
    count: 1,
    why: "a zero timeout gives every attempt up before its call is sent",
};

const TIMEOUT_CEILING_MILLIS: End = End {
    count: 10 * 60 * 1000,
    why: "one attempt past ten minutes outwaits every edge's deadline many times over, and a \
          value that long is a unit slip more often than a choice (`600` meant as seconds)",
};

const RETRIES_CEILING: End = End {
    count: 10,
    why: "past ten, one caller sends a failing dependency eleven calls, the hammering a breaker \
          exists to stop",
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
    fn retries_that_are_no_whole_number_are_refused_naming_what_they_take() {
        for value in [parse_quote!("3"), parse_quote!(-1), parse_quote!(1.5)] {
            assert_eq!(
                refused(resilient_retries(&value)),
                "#[resilient] `retries` takes a whole number, e.g. `retries = 3`"
            );
        }
    }
}
