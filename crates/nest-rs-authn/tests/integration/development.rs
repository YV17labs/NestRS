//! Covers `src/development.rs` — a test process, which `TestApp` declares,
//! boots an app listing the guard and serves the route it guards; a deployed
//! one refuses the boot of an app listing it, and a request the guard reaches
//! outside development is refused.

use nest_rs_authn::DevelopmentOnlyGuard;
use nest_rs_config::Environment;
use nest_rs_core::module;
use nest_rs_http::{HttpModule, StatusCode, controller, routes};
use nest_rs_testing::{HeadlessApp, LogCapture, TestApp};

#[controller(path = "/dev")]
#[use_guards(DevelopmentOnlyGuard)]
struct SeedController;

#[routes]
impl SeedController {
    #[post("/seed")]
    #[public]
    async fn seed(&self) -> &'static str {
        "seeded"
    }
}

#[module(
    imports = [HttpModule],
    providers = [DevelopmentOnlyGuard, SeedController],
)]
struct DevApp;

#[tokio::test]
async fn a_test_process_boots_the_guard_and_serves_its_route() {
    let app = TestApp::for_module::<DevApp>()
        .await
        .expect("a test process boots");

    let response = app.http().post("/dev/seed").send().await;

    response.assert_status_is_ok();
    response.assert_text("seeded").await;
}

#[module(providers = [DevelopmentOnlyGuard])]
struct GuardListingApp;

/// Whatever lists the guard, a process declaring a deployment refuses its
/// boot naming the variable that decides it.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_deployed_process_refuses_the_boot_of_an_app_listing_the_guard() {
    // Sync on purpose: figment::Jail scopes env to one thread, so the tokio
    // runtime is opened inside the closure.
    figment::Jail::expect_with(|jail| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        for declared in ["staging", "production"] {
            jail.set_env(Environment::var_name(), declared);

            let refused = runtime.block_on(async {
                let app = HeadlessApp::builder()
                    .module::<GuardListingApp>()
                    .build_headless()
                    .await
                    .expect("the app builds; its init hooks refuse");
                app.init()
                    .await
                    .expect_err("a deployed process refuses the boot")
            });

            let said = format!("{refused:#}");
            assert!(said.contains(&Environment::var_name()), "{said}");
            assert!(said.contains(&format!("declares {declared}")), "{said}");
        }
        Ok(())
    });
}

/// A process that stops declaring development after its boot is refused at
/// the request, and the edge files the refusal once on this crate's target,
/// naming why.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_request_outside_development_is_refused_and_filed_on_the_authn_target() {
    figment::Jail::expect_with(|jail| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let logs = LogCapture::install();
        jail.set_env(Environment::var_name(), "test");
        let app = runtime
            .block_on(TestApp::for_module::<DevApp>())
            .expect("a test process boots");

        jail.set_env(Environment::var_name(), "production");
        runtime.block_on(async {
            app.http()
                .post("/dev/seed")
                .send()
                .await
                .assert_status(StatusCode::FORBIDDEN);
        });

        let line = logs.expect_one(nest_rs_authn::TARGET, "guard denied the request");
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("reason").as_deref(), Some("outside_development"));
        Ok(())
    });
}
