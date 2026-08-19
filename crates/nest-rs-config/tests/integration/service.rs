//! What `ConfigService` claims on behalf of the type it is reading for.
//!
//! `<PREFIX>_<DOMAIN>__<KEY>` is a flat, process-global name space and nothing
//! owned uniqueness over it. Several types sharing a `<DOMAIN>` is deliberate —
//! `nest-rs-authn` ships three `authn` configs — but two types reading one
//! *variable* means a deployment setting it configures whichever happened to
//! read it, both silently. Their key sets were disjoint by accident of the
//! current fields, which is a fact about today rather than a property anything
//! held.
//!
//! Each test owns its process (nextest), because the claim registry is
//! process-global by construction: it is what a boot builds up across every
//! config an app loads.

use nest_rs_config::{Config, ConfigError, ConfigService, Result, config, var_name};

#[config(namespace = "claims")]
#[derive(Clone, Debug, Default)]
struct First {
    token: Option<String>,
}

impl Config for First {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            token: env.get("TOKEN").or(base.token),
        })
    }
}

/// A second type in the same domain, reading a **different** key — the shape
/// `nest-rs-authn` ships three of, and the one that must keep working.
#[config(namespace = "claims")]
#[derive(Clone, Debug, Default)]
struct Sibling {
    audience: Option<String>,
}

impl Config for Sibling {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            audience: env.get("AUDIENCE").or(base.audience),
        })
    }
}

/// A second type reading the **same** key. Nothing in the type system, the
/// macro or the namespace grammar can see this — only the read can.
#[config(namespace = "claims")]
#[derive(Clone, Debug, Default)]
struct Contender {
    token: Option<String>,
}

impl Config for Contender {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            token: env.get("TOKEN").or(base.token),
        })
    }
}

#[test]
fn two_types_may_share_a_domain_while_their_keys_differ() {
    First::load().expect("the first claims its key");
    Sibling::load().expect("a sibling in the same domain claims another");
}

#[test]
fn two_types_may_not_read_one_variable() {
    First::load().expect("the first claims its key");
    let err = Contender::load().expect_err("the second reads a claimed variable");
    let ConfigError::ContestedVariable {
        var,
        owner,
        claimant,
    } = &err
    else {
        panic!("expected a contested-variable error, got {err}");
    };
    assert_eq!(var, &nest_rs_config::var_name("claims", "TOKEN"));

    // Rendered, not only destructured. The message shipped with three runs of
    // ten spaces in it — wrapped-source continuations that reached the operator
    // verbatim — because the only assertion read the variant's fields and never
    // its `Display`. It is the sole artefact of this whole check anyone outside
    // the process ever sees.
    let rendered = err.to_string();
    assert!(
        !rendered.contains("  "),
        "the message is what an operator reads, and it carries no source \
         indentation: {rendered:?}",
    );
    assert!(
        rendered.contains(&nest_rs_config::var_name("claims", "TOKEN")),
        "and it names the variable: {rendered}",
    );
    assert!(
        owner.ends_with("First"),
        "the first reader is named: {owner}"
    );
    assert!(
        claimant.ends_with("Contender"),
        "and so is the second: {claimant}",
    );
}

/// Resolving one type twice is a boot loading it twice, not a collision.
#[test]
fn a_type_may_read_its_own_variable_again() {
    First::load().expect("first load");
    First::load().expect("a second load of the same type is not a contest");
}

/// The claim is the **resolved name**, so a key reached through a `const` or a
/// sub-reader counts exactly as a literal does — which is what a scan over the
/// source cannot say, and why this check is not one.
#[test]
fn a_key_read_through_a_const_is_claimed_like_any_other() {
    const KEY: &str = "TOKEN";

    #[config(namespace = "claims_const")]
    #[derive(Clone, Debug, Default)]
    struct ViaConst {
        token: Option<String>,
    }

    impl Config for ViaConst {
        fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                token: env.get(KEY).or(base.token),
            })
        }
    }

    #[config(namespace = "claims_const")]
    #[derive(Clone, Debug, Default)]
    struct AlsoViaConst {
        token: Option<String>,
    }

    impl Config for AlsoViaConst {
        fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                token: env.get(KEY).or(base.token),
            })
        }
    }

    ViaConst::load().expect("first load");
    let err = AlsoViaConst::load().expect_err("a const key is claimed like a literal");
    assert!(
        matches!(err, ConfigError::ContestedVariable { .. }),
        "got {err}",
    );
}

/// Citing a variable in a message is not reading it: `var_name` is what an
/// error string calls the variable, sometimes as a glob (`TLS_*`), and a claim
/// on that would be a claim on a name nothing sets.
#[test]
fn citing_a_variable_is_not_claiming_it() {
    #[config(namespace = "claims_cite")]
    #[derive(Clone, Debug, Default)]
    struct Citer {
        token: Option<String>,
    }

    impl Config for Citer {
        fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
            let _cited = env.var_name("TOKEN");
            Ok(Self { token: base.token })
        }
    }

    #[config(namespace = "claims_cite")]
    #[derive(Clone, Debug, Default)]
    struct Reader {
        token: Option<String>,
    }

    impl Config for Reader {
        fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                token: env.get("TOKEN").or(base.token),
            })
        }
    }

    Citer::load().expect("citing claims nothing");
    Reader::load().expect("so the real reader still gets the variable");
}

/// The boundary the module doc states: the registry sits on `ConfigService`,
/// so the free [`env_var`] is outside it.
///
/// Pinned rather than left implicit, because the doc says so in words and a
/// sentence about a refusal is worth exactly what proves it. Whoever decides
/// the free function *should* be covered — an owner question, since it is
/// called from places with no config in flight at all — will find this test
/// red, which is the point: the boundary moves in the open, with the paragraph
/// that describes it.
#[test]
fn the_free_reader_is_outside_the_registry() {
    #[config(namespace = "claims_free")]
    #[derive(Clone, Debug, Default)]
    struct Owner {
        token: Option<String>,
    }

    impl Config for Owner {
        fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                token: env.get("TOKEN").or(base.token),
            })
        }
    }

    #[config(namespace = "claims_free_borrower")]
    #[derive(Clone, Debug, Default)]
    struct Borrower {
        token: Option<String>,
    }

    impl Config for Borrower {
        fn from_env(_env: &ConfigService, base: Self) -> Result<Self> {
            // The spelling `docs/configuration/env-cascade` teaches for a
            // borrow — built, never spelled, but still not this type's domain.
            let borrowed = nest_rs_config::env_var(&var_name("claims_free", "TOKEN"));
            Ok(Self {
                token: borrowed.or(base.token),
            })
        }
    }

    Borrower::load().expect("the free reader claims nothing");
    Owner::load().expect("so the owner's own read is uncontested");
}
