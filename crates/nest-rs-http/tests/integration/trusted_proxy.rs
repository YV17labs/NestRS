//! `src/trusted_proxy.rs`: the entry grammar `trusted_proxy!` reads a literal
//! with is a copy of the type's own, since the macro crate cannot depend on
//! this one, so one corpus runs through both.

use std::panic::{AssertUnwindSafe, catch_unwind};

use nest_rs_http::TrustedProxy;

/// Every form an entry takes, and every way one is refused.
const CORPUS: &[&str] = &[
    "10.42.0.0/16",
    "10.42.0.0",
    "10.0.0.1/32",
    "fd00::/8",
    "2001:db8::1",
    "::ffff:10.0.0.1",
    "::ffff:10.0.0.0/104",
    "",
    "proxy.example",
    "10.0.0.oops",
    "10.0.0.0/",
    "/8",
    "10.0.0.0/+8",
    "10.0.0.0/-8",
    "10.0.0.0/8/8",
    "10.0.0.0/255.0.0.0",
    "10.0.0.0/256",
    "10.0.0.0 /8",
    "[::1]",
    "10.0.0.0/33",
    "fd00::/129",
    "0.0.0.0/0",
    "::/0",
    "::ffff:0:0/96",
    "10.0.0.1/8",
    "fd00::1/8",
];

/// What the constant `trusted_proxy!` expands to builds from `text`, or the
/// sentence its evaluation panics with, which fails the build.
fn as_the_macro_builds_it(text: &str) -> Result<TrustedProxy, String> {
    let parsed = nest_rs_codegen::__private::trusted_proxy_entry(text);
    catch_unwind(AssertUnwindSafe(|| {
        nest_rs_http::__private::trusted_proxy_literal(parsed)
    }))
    .map_err(|payload| match payload.downcast::<String>() {
        Ok(said) => *said,
        Err(payload) => payload
            .downcast_ref::<&str>()
            .map(|said| (*said).to_owned())
            .unwrap_or_default(),
    })
}

/// The two copies agree on every entry, and the build's refusal is the type's
/// sentence with the macro in front.
#[test]
fn the_macro_reads_every_entry_as_the_type_reads_it() {
    for text in CORPUS {
        match (text.parse::<TrustedProxy>(), as_the_macro_builds_it(text)) {
            (Ok(read), Ok(built)) => assert_eq!(read, built, "{text:?}"),
            (Err(refused), Err(said)) => {
                assert_eq!(said, format!("`trusted_proxy!`: {refused}"), "{text:?}");
            }
            (read, built) => panic!("{text:?}: the type read {read:?}, the macro {built:?}"),
        }
    }
}
