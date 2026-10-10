//! Integration tests mirroring `src/`; `entry`, `format` and `config` carry
//! their own unit tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod interceptor;
mod module;

use std::future::Future;
use std::time::Duration;

use nest_rs_config::Environment;
use nest_rs_core::Module;
use nest_rs_http::{controller, routes};
use nest_rs_server_timing::{ServerTimingModule, Timings};
use nest_rs_testing::{TestApp, TestResponse};

#[controller(path = "/")]
pub(crate) struct ProbeController;

#[routes]
impl ProbeController {
    /// Records one sub-step the documented way, through [`Timings`].
    #[get("/")]
    #[public]
    async fn index(&self, req: &nest_rs_http::poem::Request) -> String {
        if let Some(timings) = req.extensions().get::<Timings>() {
            timings.record("db", Duration::from_millis(3));
        }
        "ok".into()
    }
}

/// The documented import, bare.
#[nest_rs_core::module(imports = [ServerTimingModule], providers = [ProbeController])]
pub(crate) struct BareModule;

/// The `server-timing` header a response carries, if any.
pub(crate) fn server_timing(resp: &TestResponse) -> Option<String> {
    resp.0.header("server-timing").map(str::to_owned)
}

/// Boots `M` and returns the `server-timing` header its probe answers with.
pub(crate) async fn probe_timing<M: Module + 'static>() -> Option<String> {
    let app = TestApp::for_module::<M>().await.expect("the import boots");
    let resp = app.http().get("/").send().await;
    resp.assert_status_is_ok();
    server_timing(&resp)
}

/// Runs `boot` in a process that declares `profile`, with `vars` set beside it.
///
/// The profile is read off the process environment, which `figment::Jail`
/// scopes to its closure, so the runtime opens inside it.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
pub(crate) fn in_profile<F, T>(
    profile: &str,
    vars: &[(String, &str)],
    boot: impl FnOnce() -> F,
) -> T
where
    F: Future<Output = T>,
{
    let mut out = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(Environment::var_name(), profile);
        for (name, value) in vars {
            jail.set_env(name, value);
        }
        out = Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(boot()),
        );
        Ok(())
    });
    out.expect("the jailed boot ran")
}
