//! Load the project's `.env` cascade for e2e: the harness reads backend URLs
//! before any `ConfigModule` exists, from a crate dir rather than the root.

use std::sync::Once;

use nest_rs_config::{Environment, load_cascade};

/// Load the nearest project `.env` once per process, set-if-absent and bounded
/// to the git repo, defaulting `<PREFIX>_ENV=test` first.
///
/// Every harness entry point calls it first: `<PREFIX>_ENV` defaulted after
/// this `Once` ran would select the wrong cascade.
#[expect(
    clippy::disallowed_methods,
    reason = "the harness defaults the variable the cascade selects on, set-if-absent"
)]
pub fn load_project_env() {
    static LOADED: Once = Once::new();
    LOADED.call_once(|| {
        // Before the `.env` lookup: env-aware defaults must see `test` even
        // with no file found.
        let env_var = Environment::var_name();
        if std::env::var_os(&env_var).is_none() {
            // SAFETY: not discharged by a single-threaded-bootstrap claim,
            // and it is worth saying why rather than repeating one. This runs
            // from `TestApp::builder` and `EphemeralDatabase::create`, i.e.
            // from inside a `#[tokio::test]` body — after the runtime built
            // its pool. Suites do run it under `flavor = "multi_thread"`, and
            // one connects to Postgres by hostname immediately after, which is
            // the `ToSocketAddrs` reader `std::env::set_var`'s own docs name.
            //
            // What makes it sound *here* is the `Once` plus the position: this
            // is the first statement of the first harness entry point a test
            // process reaches, so the write lands before any task this process
            // spawns can read the environment, and it never runs again. The
            // residual race is with a reader spawned by an *earlier* test in
            // the same process, which nextest's process-per-test model rules
            // out.
            //
            // The write is what `<PREFIX>_LOG` and `OpenTelemetry::init` read
            // through bare `std::env::var`, which is the whole reason a
            // resolution-only cascade is not enough. `load_cascade` below does
            // one further `unsafe set_var` per key under its own note — so
            // this is not the crate's only unsafe, only its first.
            #[expect(unsafe_code, reason = "set_var before any thread reads the environment, per the SAFETY note above")]
            unsafe {
                std::env::set_var(&env_var, "test")
            };
        }
        let Ok(mut dir) = std::env::current_dir() else {
            return;
        };
        loop {
            if dir.join(".env").is_file() {
                load_cascade(&dir, Environment::from_env());
                return;
            }
            if dir.join(".git").exists() || !dir.pop() {
                return;
            }
        }
    });
}
