//! The one grammar the framework reads an environment boolean with.
//!
//! Declared in the kernel, ungated, for the reason
//! [`EnvPrefix`](crate::EnvPrefix) is: the two values a process reads *before*
//! anything is configured — what the variables are called, and what `true`
//! looks like — cannot live in the crate that does the configuring.
//! `nest_rs_core::logging` reads `<PREFIX>_LOG_SOURCE_LOCATION` as the
//! subscriber installs, before a container exists.
//!
//! It sat inside that `logging` module for a while, described as "canonical
//! env-flag grammar for every framework boolean var" — and the module is behind
//! a Cargo feature an embedder may switch off, so the crate that actually reads
//! *every* `<PREFIX>_<NS>__<KEY>` boolean a deployment writes could not reach
//! it and re-typed the eight words instead. One grammar, two spellings, and the
//! day either grows `y` or `enabled` a deployment's `NESTRS_HTTP__COMPRESSION`
//! and its `NESTRS_LOG_SOURCE_LOCATION` diverge with nothing to say so.

/// `1`/`true`/`yes`/`on` ⇒ `true`, `0`/`false`/`no`/`off` ⇒ `false`, anything
/// else ⇒ `None`. Case-insensitive, trimmed.
///
/// The caller applies its own answer for the unrecognised and absent cases —
/// source location defaults off, an access log defaults on, a `#[config]` field
/// reports the value back as a boot error — which is what keeps the truthy and
/// falsy vocabulary itself in one place.
pub fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_case_insensitive_and_trimmed() {
        for truthy in ["1", "true", "TRUE", " yes ", "On"] {
            assert_eq!(parse_bool(truthy), Some(true), "{truthy}");
        }
        for falsy in ["0", "false", "FALSE", " no ", "Off"] {
            assert_eq!(parse_bool(falsy), Some(false), "{falsy}");
        }
    }

    /// Unrecognised is `None`, never `false`: the two mean different things to
    /// every caller — one is "the operator wrote something we do not read", the
    /// other is "the operator said no".
    #[test]
    fn an_unrecognised_value_is_not_a_no() {
        for other in ["", "y", "enabled", "2", "maybe"] {
            assert_eq!(parse_bool(other), None, "{other}");
        }
    }
}
