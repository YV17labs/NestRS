//! The fail-secure boot refusal for an imperative `mount(...)`.
//!
//! Every controller route is shaped by `#[routes]` (which runs the global guard
//! pool) and every self-mount declares an `EdgePosture`. An imperative
//! [`HttpTransport::mount`] is neither: it hands the transport an opaque poem
//! endpoint, so when a global guard pool is active that endpoint is the one hole
//! the pool cannot cover. Strict mode — the default — refuses to boot.
//!
//! Documented on `/http/configuration/` under *Fail-secure boot*, and until this
//! module existed it was the one fail-closed guard in the release with no proof
//! it ever fired: nothing in the corpus reaches `mount(...)`, so no QA pass
//! could trigger it by following a page.

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{GlobalGuardsActive, HttpTransport, controller, routes};

#[controller(path = "/hello")]
struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    #[public]
    async fn hello(&self) -> &'static str {
        "hello"
    }
}

#[module(providers = [HelloController])]
struct HelloModule;

/// The three axes the check reads. Named rather than positional: the violating
/// case and the two controls differ by one field each, and as bare booleans a
/// transposed pair would silently retarget a test at the case beside it.
///
/// `Default` is the violation — strict mode, an imperative mount, a global pool
/// — so each control reads as the one thing it relaxes.
struct Setup {
    strict: bool,
    mounted: bool,
    /// Seeds the marker `use_guards_global` provides — the transport reads that
    /// marker, not the `Guard` trait, which is what keeps this crate below
    /// `nest-rs-guards`.
    guards: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            strict: true,
            mounted: true,
            guards: true,
        }
    }
}

async fn configure(setup: Setup) -> anyhow::Result<()> {
    let mut app = App::builder().module::<HelloModule>();
    if setup.guards {
        app = app.provide(GlobalGuardsActive);
    }
    let app = app.build().await.expect("module boots");
    let mut transport = HttpTransport::new().fail_secure_strict(setup.strict);
    if setup.mounted {
        transport = transport.mount("/raw", |_| poem::endpoint::make_sync(|_| "raw"));
    }
    transport.configure(app.container()).await
}

#[tokio::test]
async fn an_imperative_mount_under_global_guards_refuses_to_boot() {
    let err = configure(Setup::default())
        .await
        .expect_err("an unshapeable endpoint beside a global guard pool must not mount");
    let text = err.to_string();
    assert!(text.contains("fail-secure"), "got: {text}");
    assert!(
        text.contains("/raw"),
        "the refusal names the offending mount, or the reader cannot find it: {text}",
    );
    assert!(
        text.contains("fail_secure_strict"),
        "and names the opt-out it is the strict half of: {text}",
    );
}

#[tokio::test]
async fn the_opt_out_downgrades_the_refusal_to_a_warn() {
    // `fail_secure_strict(false)` is a deliberate choice the deployment is
    // entitled to; it must boot, not fail differently.
    configure(Setup {
        strict: false,
        ..Setup::default()
    })
    .await
    .expect("the documented opt-out boots the same app");
}

#[tokio::test]
async fn a_mount_without_a_global_guard_pool_is_not_a_violation() {
    // Nothing to bypass: an app with no global pool gates per controller, and
    // refusing here would break every app that mounts a metrics endpoint.
    configure(Setup {
        guards: false,
        ..Setup::default()
    })
    .await
    .expect("no global guard pool, no fail-secure violation");
}

#[tokio::test]
async fn controllers_alone_boot_under_strict_mode() {
    // The check must fire on the *mount*, not on the guard pool: a guarded app
    // with no imperative mount is the ordinary case and stays bootable.
    configure(Setup {
        mounted: false,
        ..Setup::default()
    })
    .await
    .expect("shaped routes are covered by the pool by construction");
}
