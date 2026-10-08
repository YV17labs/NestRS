//! What `ConfigService` claims on behalf of the type it is reading for. Each
//! test owns its process (nextest): the claim registry is process-global.

use nest_rs_config::{Config, ConfigError, ConfigService, Result, config, var_name};

#[config(namespace = "claims")]
#[derive(Clone, Debug, Default)]
struct First {
    token: Option<String>,
}

impl Config for First {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            token: env.get("TOKEN")?.or(base.token),
        })
    }
}

/// A second type reading the **same** variable, through a reader it opened on
/// `First`'s domain.
#[config(namespace = "claims_contender")]
#[derive(Clone, Debug, Default)]
struct Contender {
    token: Option<String>,
}

impl Config for Contender {
    fn from_env(_env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            token: ConfigService::for_namespace("claims")
                .get("TOKEN")?
                .or(base.token),
        })
    }
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
/// sub-reader counts exactly as a literal does.
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
                token: env.get(KEY)?.or(base.token),
            })
        }
    }

    #[config(namespace = "claims_const_borrower")]
    #[derive(Clone, Debug, Default)]
    struct AlsoViaConst {
        token: Option<String>,
    }

    impl Config for AlsoViaConst {
        fn from_env(_env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                token: ConfigService::for_namespace("claims_const")
                    .get(KEY)?
                    .or(base.token),
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

/// Citing a variable through `var_name` is not reading it.
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

    #[config(namespace = "claims_cite_reader")]
    #[derive(Clone, Debug, Default)]
    struct Reader {
        token: Option<String>,
    }

    impl Config for Reader {
        fn from_env(_env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                token: ConfigService::for_namespace("claims_cite")
                    .get("TOKEN")?
                    .or(base.token),
            })
        }
    }

    Citer::load().expect("citing claims nothing");
    Reader::load().expect("so the real reader still gets the variable");
}

/// The registry sits on `ConfigService`, so the free [`env_var`] is outside it.
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
                token: env.get("TOKEN")?.or(base.token),
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
            let borrowed = nest_rs_config::env_var(&var_name("claims_free", "TOKEN"));
            Ok(Self {
                token: borrowed.or(base.token),
            })
        }
    }

    Borrower::load().expect("the free reader claims nothing");
    Owner::load().expect("so the owner's own read is uncontested");
}
