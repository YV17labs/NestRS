//! The prefix every framework environment variable carries — `NESTRS` unless
//! the application declares its own with [`env_prefix!`](crate::env_prefix!).
//!
//! Two shapes sit under it, and both come from here so a rename can never do
//! half the job:
//!
//! - **framework-wide** — `<PREFIX>_ENV`, `<PREFIX>_LOG*`, built by
//!   [`EnvPrefix::var`];
//! - **namespaced config** — `<PREFIX>_<DOMAIN>__<KEY>`, built by
//!   `nest_rs_config::var_name` on top of the same value.
//!
//! # Why a link-time declaration
//!
//! The prefix has to be known before anything reads the environment:
//! `<PREFIX>_ENV` selects the `.env` cascade, and the console subscriber reads
//! `<PREFIX>_LOG` before `main` has built anything. A setter would therefore
//! carry an ordering rule nobody can verify — call it too late and the app
//! silently resolves nothing. `inventory` removes the ordering question
//! entirely: the declaration is a link-time fact, already true when the first
//! read happens, wherever in the binary it is written.
//!
//! Resolution is cached in a `OnceLock<&'static str>`, so every later read is a
//! pointer load and no allocation is added to any path that was allocation-free.

use std::sync::OnceLock;

/// One `env_prefix!` declaration, collected at link time.
///
/// Constructed only by the [`env_prefix!`](crate::env_prefix!) macro — an app
/// never names this type. It is public because the macro expansion must, and
/// carries the declaring module so a conflict can name both sites.
pub struct EnvPrefixDecl {
    prefix: &'static str,
    declared_in: &'static str,
}

impl EnvPrefixDecl {
    /// Wrap a validated prefix literal with the module that declared it.
    #[doc(hidden)]
    pub const fn new(prefix: &'static str, declared_in: &'static str) -> Self {
        Self {
            prefix,
            declared_in,
        }
    }
}

inventory::collect!(EnvPrefixDecl);

/// The active environment-variable prefix.
pub struct EnvPrefix;

impl EnvPrefix {
    /// What an application gets without declaring anything.
    pub const DEFAULT: &'static str = "NESTRS";

    /// The active prefix — the declared one, or [`DEFAULT`](Self::DEFAULT).
    ///
    /// Resolved once per process and cached; the scan below never runs twice.
    pub fn current() -> &'static str {
        static RESOLVED: OnceLock<&'static str> = OnceLock::new();
        RESOLVED.get_or_init(resolve)
    }

    /// A framework-wide variable name: `ENV` ⇒ `NESTRS_ENV`, or `ACME_ENV`
    /// under `env_prefix!("ACME")`.
    ///
    /// Namespaced config variables do **not** go through here — they carry a
    /// domain segment and are built by `nest_rs_config::var_name`.
    pub fn var(name: &str) -> String {
        format!("{}_{name}", Self::current())
    }
}

/// Read the single declaration out of the link-time registry.
///
/// Repeating the *same* prefix is allowed on purpose: a workspace where both
/// the shared library crate and a binary declare it is consistent, and refusing
/// it would push apps into inventing a "who owns the declaration" rule. Two
/// **different** prefixes have no defensible winner — one half of the app would
/// read variables the other half never writes — so that aborts, naming both
/// sites.
fn resolve() -> &'static str {
    let mut declarations = inventory::iter::<EnvPrefixDecl>.into_iter();
    let Some(first) = declarations.next() else {
        return EnvPrefix::DEFAULT;
    };
    for decl in declarations {
        assert!(
            decl.prefix == first.prefix,
            "conflicting `env_prefix!` declarations: `{}` in `{}` and `{}` in `{}`. \
             An application declares exactly one environment-variable prefix.",
            first.prefix,
            first.declared_in,
            decl.prefix,
            decl.declared_in,
        );
    }
    first.prefix
}

/// Compile-time shape check for the [`env_prefix!`](crate::env_prefix!) literal,
/// so a lowercase or trailing-underscore prefix is a build error rather than a
/// deployment where nothing resolves.
///
/// A `const fn` because the macro calls it from a `const _: () = …` item: the
/// diagnostic then points at the literal, and no check reaches runtime.
#[doc(hidden)]
pub const fn assert_env_prefix(prefix: &str) {
    let bytes = prefix.as_bytes();
    if bytes.is_empty() {
        panic!("env_prefix! must not be empty");
    }
    if !bytes[0].is_ascii_uppercase() {
        panic!("env_prefix! must start with an uppercase ASCII letter, e.g. env_prefix!(\"ACME\")");
    }
    if bytes[bytes.len() - 1] == b'_' {
        panic!(
            "env_prefix! must not end with `_` — the framework adds the separator \
             (\"ACME\" yields ACME_ENV, ACME_DATABASE__URL)"
        );
    }
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if !(byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_') {
            panic!(
                "env_prefix! takes uppercase ASCII letters, digits and underscores only, \
                 e.g. env_prefix!(\"ACME\")"
            );
        }
        i += 1;
    }
}

/// Declare this application's environment-variable prefix, replacing `NESTRS`.
///
/// Write it **once**, at the root of a crate every binary of the project links
/// — the shared library crate in a workspace, `main.rs` in a single-binary app.
/// Every framework variable follows: `ACME_ENV`, `ACME_LOG`,
/// `ACME_DATABASE__URL`, `ACME_HTTP__PORT`.
///
/// ```
/// nest_rs_core::env_prefix!("ACME");
/// # fn main() {}
/// ```
///
/// The literal is checked at compile time (uppercase ASCII, digits and
/// underscores, no trailing `_` — the framework supplies the separator).
///
/// # Put it where the tests see it
///
/// The declaration is a property of the *binary*, so a prefix declared in
/// `main.rs` is invisible to the crate's own test binaries, which would then
/// resolve `NESTRS_*` while the app resolves `ACME_*`. Declaring it in a
/// library crate the binaries and the tests both link keeps them in step.
#[macro_export]
macro_rules! env_prefix {
    ($prefix:literal) => {
        const _: () = $crate::assert_env_prefix($prefix);

        $crate::inventory::submit! {
            $crate::EnvPrefixDecl::new($prefix, ::core::module_path!())
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // No `env_prefix!` in this binary, so the default stands — and the same
    // read twice must give the same answer (the cache is not re-resolving).
    #[test]
    fn an_undeclared_prefix_is_nestrs() {
        assert_eq!(EnvPrefix::current(), "NESTRS");
        assert_eq!(EnvPrefix::current(), EnvPrefix::DEFAULT);
    }

    #[test]
    fn var_joins_the_prefix_with_a_single_underscore() {
        assert_eq!(EnvPrefix::var("ENV"), "NESTRS_ENV");
        assert_eq!(EnvPrefix::var("LOG_FORMAT"), "NESTRS_LOG_FORMAT");
    }

    // The shape rules are a `const fn`, so the real proof is a compile
    // failure; this pins the accepting half, which is what a valid literal
    // relies on.
    #[test]
    fn the_shape_check_accepts_the_documented_forms() {
        assert_env_prefix("ACME");
        assert_env_prefix("MY_PROJECT");
        assert_env_prefix("ACME2");
        assert_env_prefix("NESTRS");
    }
}
