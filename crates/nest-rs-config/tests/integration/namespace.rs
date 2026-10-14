//! Covers `src/namespace.rs` — a namespace belongs to one type.

use nest_rs_config::{
    Config, ConfigError, ConfigModule, ConfigService, Namespaced, Result, config,
};

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

#[config(namespace = "drawer")]
#[derive(Clone, Debug, Default)]
struct DrawerConfig {
    label: Option<String>,
}

impl Config for DrawerConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            label: env.get("LABEL")?.or(base.label),
        })
    }
}

/// What a package's binding of `drawer` to its own lock port would declare.
#[config(namespace = "drawer__lock")]
#[derive(Clone, Debug, Default)]
struct DrawerLockConfig {
    lease: Option<u16>,
}

impl Config for DrawerLockConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            lease: env.parse("LEASE")?.or(base.lease),
        })
    }
}

#[nest_rs_core::instance]
struct Lock;

mod left {
    #[nest_rs_core::instance]
    pub(super) struct Twin;
}

mod right {
    #[nest_rs_core::instance]
    pub(super) struct Twin;
}

#[nest_rs_core::module]
struct Drawers;

#[nest_rs_core::module(imports = [ConfigModule::setup::<Drawers, _, _>((Lock, None::<DrawerConfig>))])]
struct LockDrawer;

#[nest_rs_core::module(imports = [
    ConfigModule::setup::<Drawers, _, _>((left::Twin, None::<DrawerConfig>)),
])]
struct TwinDrawer;

/// The boot of `M` refused as a shared instance: its marker, its namespace and
/// the claims beside it, then the sentence.
fn shared_instance<M: nest_rs_core::Module + 'static>()
-> (&'static str, String, Vec<&'static str>, String) {
    let booted = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("test runtime")
        .block_on(nest_rs_core::App::builder().module::<M>().build());
    let Err(error) = booted else {
        panic!("an instance sharing its namespace must not boot");
    };
    match error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ConfigError>())
    {
        Some(
            refused @ ConfigError::SharedInstance {
                instance,
                namespace,
                claims,
            },
        ) => (
            instance,
            namespace.clone(),
            claims.clone(),
            refused.to_string(),
        ),
        _ => panic!("expected a shared-instance refusal, got {error:#}"),
    }
}

/// A package binding `drawer` to its own lock port declares `drawer__lock`, a
/// word no reserved list can know: the instance `Lock` would read its variables.
#[test]
fn an_instance_whose_namespace_a_linked_config_declares_is_refused() {
    let (instance, namespace, claims, rendered) = shared_instance::<LockDrawer>();
    assert!(instance.ends_with("::Lock"), "{rendered}");
    assert_eq!(namespace, "drawer__lock");
    assert_eq!(claims.len(), 1, "{rendered}");
    assert!(claims[0].ends_with("::DrawerLockConfig"), "{rendered}");
    assert!(rendered.ends_with("Rename the instance."), "{rendered}");
    DrawerLockConfig::load().expect("the config keeps its own namespace");
}

/// Two crates each declaring `Twin` read one name: the variables under
/// `drawer__twin` could not say which they configure.
#[test]
fn two_markers_reading_one_name_are_refused_at_first_use() {
    let (instance, namespace, claims, rendered) = shared_instance::<TwinDrawer>();
    assert!(instance.ends_with("left::Twin"), "{rendered}");
    assert_eq!(namespace, "drawer__twin");
    assert_eq!(claims.len(), 1, "{rendered}");
    assert!(claims[0].ends_with("right::Twin"), "{rendered}");
}
