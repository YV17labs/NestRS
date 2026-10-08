//! The fail-secure boot refusal for an imperative [`HttpTransport::mount`] under
//! an active global guard pool, which cannot cover it.

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

/// The three axes the check reads. `Default` is the violation — strict mode, an
/// imperative mount, a global pool — so each control relaxes one field.
struct Setup {
    strict: bool,
    mounted: bool,
    /// Seeds the marker `use_guards_global` provides.
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
    let logs = nest_rs_testing::LogCapture::install();
    configure(Setup {
        strict: false,
        ..Setup::default()
    })
    .await
    .expect("the documented opt-out boots the same app");

    let event = logs.expect_one(
        "nest_rs::http",
        "imperative mounts bypass the global guard pool",
    );
    assert_eq!(event.level, "warn");
    assert!(
        event.field("paths").is_some(),
        "the event names which mounts are unguarded, got {:?}",
        event.fields,
    );
    assert!(
        event
            .field("hint")
            .is_some_and(|h| h.contains("controller")),
        "and carries the remedy, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_mount_without_a_global_guard_pool_is_not_a_violation() {
    configure(Setup {
        guards: false,
        ..Setup::default()
    })
    .await
    .expect("no global guard pool, no fail-secure violation");
}

#[tokio::test]
async fn controllers_alone_boot_under_strict_mode() {
    configure(Setup {
        mounted: false,
        ..Setup::default()
    })
    .await
    .expect("shaped routes are covered by the pool by construction");
}
