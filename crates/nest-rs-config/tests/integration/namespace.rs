//! Covers `src/namespace.rs` — a namespace belongs to one type.

use nest_rs_config::{Config, ConfigError, ConfigService, Namespaced, Result, config};

#[config(namespace = "shared_namespace")]
#[derive(Clone, Debug, Default)]
struct Ports {
    port: Option<u16>,
}

impl Config for Ports {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            port: env.parse("PORT")?.or(base.port),
        })
    }
}

#[config(namespace = "shared_namespace")]
#[derive(Clone, Debug, Default)]
struct Hosts {
    host: Option<String>,
}

impl Config for Hosts {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            host: env.get("HOST")?.or(base.host),
        })
    }
}

fn assert_refused_naming_both(err: ConfigError) {
    let rendered = err.to_string();
    let ConfigError::SharedNamespace {
        namespace,
        declarations,
    } = &err
    else {
        panic!("expected a shared-namespace refusal, got {rendered}");
    };
    assert_eq!(*namespace, "shared_namespace");
    assert_eq!(declarations.len(), 2, "{rendered}");
    assert!(
        declarations.iter().any(|d| d.ends_with("::Ports"))
            && declarations.iter().any(|d| d.ends_with("::Hosts")),
        "both declarations are named: {rendered}",
    );
    assert!(
        !rendered.contains("  "),
        "no source indentation: {rendered:?}"
    );
}

#[test]
fn a_namespace_two_structs_declare_is_refused_at_the_first_read() {
    assert_refused_naming_both(Ports::load().expect_err("the first read is refused"));
}

#[test]
fn and_at_the_read_of_the_other() {
    assert_refused_naming_both(Hosts::load().expect_err("the other read is refused too"));
}

/// A hand-written `Namespaced` files no registry entry, so its own read is
/// where it is refused beside a `#[config]` declaring the same namespace.
#[test]
fn a_hand_written_namespace_is_refused_beside_a_declared_one() {
    #[derive(Clone, Debug, Default)]
    struct Manual;

    impl Namespaced for Manual {
        const NAMESPACE: &'static str = "declared_once";
    }

    impl validator::Validate for Manual {
        fn validate(&self) -> std::result::Result<(), validator::ValidationErrors> {
            Ok(())
        }
    }

    impl Config for Manual {
        fn from_env(_env: &ConfigService, base: Self) -> Result<Self> {
            Ok(base)
        }
    }

    #[config(namespace = "declared_once")]
    #[derive(Clone, Debug, Default)]
    struct Declared {
        _token: Option<String>,
    }

    impl Config for Declared {
        fn from_env(_env: &ConfigService, base: Self) -> Result<Self> {
            Ok(base)
        }
    }

    let err = Manual::load().expect_err("refused beside the declared one");
    let rendered = err.to_string();
    assert!(
        rendered.contains("Manual") && rendered.contains("::Declared"),
        "{rendered}"
    );
    Declared::load().expect("the declared one cannot see a type that filed nothing");
}

/// A namespace one type declares is read as before — the refusal is about two.
#[test]
fn a_namespace_one_struct_declares_is_read() {
    #[config(namespace = "declared_alone")]
    #[derive(Clone, Debug, Default)]
    struct Alone {
        token: Option<String>,
    }

    impl Config for Alone {
        fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                token: env.get("TOKEN")?.or(base.token),
            })
        }
    }

    let read = nest_rs_config::read(
        &ConfigService::with_vars("declared_alone", [("TOKEN", "t")]),
        Alone::default(),
    )
    .expect("one declaration reads");
    assert_eq!(read.token.as_deref(), Some("t"));
}
