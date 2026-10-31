//! Covers `src/namespace.rs` — a namespace belongs to one type.

use nest_rs_config::{
    Config, ConfigError, ConfigModule, ConfigService, Namespaced, Result, config,
};

#[config(namespace = "shared_namespace")]
#[derive(Clone, Debug, Default)]
struct Ports {
    port: Option<u16>,
}

#[config(namespace = "shared_namespace")]
#[derive(Clone, Debug, Default)]
struct Hosts {
    host: Option<String>,
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

    #[config(namespace = "declared_once", from_env = "manual")]
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

/// What a package's binding of `drawer` to its own lock port would declare.
#[config(namespace = "drawer__lock")]
#[derive(Clone, Debug, Default)]
struct DrawerLockConfig {
    lease: Option<u16>,
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

/// The drawers of one instance, read from its environment as a bare import reads.
struct Drawers<I>(std::marker::PhantomData<fn() -> I>);

impl<I: nest_rs_core::Instance> nest_rs_core::Module for Drawers<I> {
    fn register(
        builder: nest_rs_core::ContainerBuilder,
        _: nest_rs_core::Registering<Self>,
    ) -> nest_rs_core::ContainerBuilder {
        builder
    }
    fn collect(
        builder: nest_rs_core::ContainerBuilder,
        _: nest_rs_core::Collecting<Self>,
    ) -> nest_rs_core::ContainerBuilder {
        ConfigModule::provide_feature::<I, DrawerConfig>(builder)
    }
}

#[nest_rs_core::module(imports = [Drawers::<Lock>])]
struct LockDrawer;

#[nest_rs_core::module(imports = [Drawers::<left::Twin>])]
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
