//! The prefix every framework environment variable carries — `NESTRS` unless
//! the deployment sets [`EnvPrefix::VAR`].
//!
//! Two shapes sit under it, and both come from here so a rename can never do
//! half the job:
//!
//! - **framework-wide** — `<PREFIX>_ENV`, `<PREFIX>_LOG*`, built by
//!   [`EnvPrefix::var`];
//! - **namespaced config** — `<PREFIX>_<DOMAIN>__<KEY>`, built by
//!   `nest_rs_config::var_name` on top of the same value.
//!
//! # Why the environment, and why one fixed name
//!
//! The prefix is a property of the *deployment*, not of the source: the same
//! image runs in staging and in production, and each container names its own
//! variables. So it is read from the environment, like everything else it
//! governs — and the one name that cannot itself be prefixed is this one.
//! `NESTRS_ENV_PREFIX` is therefore spelled literally, for the same reason
//! `RUST_LOG` is: it is not the application's variable, it is the bootstrap's.
//!
//! # It must be set before the process
//!
//! The prefix has to be known before anything reads the environment:
//! `<PREFIX>_ENV` selects the `.env` cascade, and the console subscriber reads
//! `<PREFIX>_LOG` before `main` has built anything. A value the *program* sets
//! for itself would therefore arrive too late — which is why the `.env` cascade
//! cannot carry it, and why `nest_rs_config` refuses one written there: a prefix
//! that shows up after the first read would rename nothing, silently.
//!
//! Resolution is cached in a `OnceLock<&'static str>`, so every later read is a
//! pointer load and no allocation is added to any path that was allocation-free.

use std::sync::OnceLock;

/// The active environment-variable prefix.
pub struct EnvPrefix;

impl EnvPrefix {
    /// What a deployment gets without setting anything.
    pub const DEFAULT: &'static str = "NESTRS";

    /// The variable the prefix itself is read from, spelled literally because
    /// it is the one name no prefix can rename.
    pub const VAR: &'static str = "NESTRS_ENV_PREFIX";

    /// The active prefix — the one the environment set, or
    /// [`DEFAULT`](Self::DEFAULT).
    ///
    /// Resolved once per process and cached; the read below never runs twice.
    /// An empty value means unset, the way an empty variable does everywhere
    /// else; a malformed one aborts rather than resolve names no operator
    /// wrote.
    pub fn current() -> &'static str {
        static RESOLVED: OnceLock<&'static str> = OnceLock::new();
        RESOLVED.get_or_init(resolve)
    }

    /// A framework-wide variable name: `ENV` ⇒ `NESTRS_ENV`, or `ACME_ENV`
    /// under `NESTRS_ENV_PREFIX=ACME`.
    ///
    /// Namespaced config variables do **not** go through here — they carry a
    /// domain segment and are built by `nest_rs_config::var_name`.
    pub fn var(name: &str) -> String {
        format!("{}_{name}", Self::current())
    }
}

/// Read the prefix once, validating its shape.
///
/// A bad value aborts here rather than propagating: every name the process is
/// about to build would carry it, so there is nothing useful to degrade to —
/// `NESTRS` would be just as wrong as the typo, and silently so. Empty is
/// unset, though: `FOO=` is how a shell says "no value", and rejecting it would
/// abort on the one spelling that means the default.
fn resolve() -> &'static str {
    let declared = std::env::var(EnvPrefix::VAR).unwrap_or_default();
    if declared.is_empty() {
        return EnvPrefix::DEFAULT;
    }
    if let Err(reason) = validate_env_prefix(&declared) {
        panic!("{}=`{declared}` {reason}", EnvPrefix::VAR);
    }
    // One leak per process, on a value that is read for the process's whole
    // life — the alternative is an allocation on every name built from it.
    String::leak(declared)
}

/// The shape a prefix must have, as the trailing half of a message reading
/// `NESTRS_ENV_PREFIX=`ac-me` <reason>`.
fn validate_env_prefix(prefix: &str) -> Result<(), &'static str> {
    let bytes = prefix.as_bytes();
    if !bytes[0].is_ascii_uppercase() {
        return Err("must start with an uppercase ASCII letter, e.g. ACME");
    }
    if bytes[bytes.len() - 1] == b'_' {
        return Err(
            "must not end with `_` — the framework supplies the separator \
             (ACME yields ACME_ENV)",
        );
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
    {
        return Err("takes uppercase ASCII letters, digits and underscores only, e.g. ACME");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The resolved value is per-process and frozen on first read, so these
    // cover the shape rules and the resolution; the environment-driven paths
    // are exercised by `nest-rs-config`'s integration suite, which owns a
    // process per prefix.
    //
    // **Neither may spell `NESTRS` as the answer.** Both did, and both failed
    // the moment the workspace was run under `NESTRS_ENV_PREFIX=ACME` — which
    // is the run that proves a rename reaches everything, so a test that can
    // only pass in one of the two processes is a test that blocks the proof.
    // The property each was written for survives the rewrite, and one of them
    // gained a property it never had: that the resolver honours a declaration.
    #[test]
    fn the_prefix_resolves_to_the_declaration_or_the_default() {
        assert_eq!(EnvPrefix::DEFAULT, "NESTRS", "the documented default");
        match std::env::var(EnvPrefix::VAR).ok().filter(|v| !v.is_empty()) {
            None => assert_eq!(EnvPrefix::current(), EnvPrefix::DEFAULT),
            Some(declared) => assert_eq!(EnvPrefix::current(), declared),
        }
    }

    #[test]
    fn var_joins_the_prefix_with_a_single_underscore() {
        for name in ["ENV", "LOG_FORMAT"] {
            assert_eq!(
                EnvPrefix::var(name).strip_prefix(EnvPrefix::current()),
                Some(format!("_{name}").as_str()),
            );
        }
    }

    #[test]
    fn the_shape_check_accepts_the_documented_forms() {
        for prefix in ["ACME", "MY_PROJECT", "ACME2", "NESTRS"] {
            assert!(
                validate_env_prefix(prefix).is_ok(),
                "{prefix} must be legal"
            );
        }
    }

    #[test]
    fn the_shape_check_rejects_what_would_resolve_nothing() {
        for prefix in ["acme", "1ACME", "ACME_", "ACME-CORP", "ACME CORP"] {
            assert!(
                validate_env_prefix(prefix).is_err(),
                "{prefix} must be rejected",
            );
        }
    }
}
