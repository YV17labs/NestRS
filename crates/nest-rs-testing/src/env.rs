//! Load the project's `.env` cascade for e2e — the harness reads backend URLs
//! via `std::env::var` before any `App` (hence `ConfigModule`) exists, and
//! tests run from a crate dir, not the project root that holds `.env`.

use std::sync::Once;

use nest_rs_config::{Environment, load_cascade};

/// Load the nearest project `.env` once per process. Set-if-absent (real env /
/// CI wins); bounded to the git repo so the framework's own `.env`-less tests
/// stay hermetic.
///
/// This `Once` is the guardian of the whole invariant: **the environment is
/// decided before any cascade read**, whichever harness entry point runs
/// first (`EphemeralDatabase::create`, `TestApp::builder`, `HeadlessApp`, …).
/// Defaulting `NESTRS_ENV=test` anywhere later would be a no-op on an already
/// consumed `Once` — the bug that made `.env.local` load (hermeticity broken)
/// and `.env.test.local` never load when a harness touched the database
/// first.
pub fn load_project_env() {
    static LOADED: Once = Once::new();
    LOADED.call_once(|| {
        // Before the `.env` lookup — even with no file found, env-aware
        // defaults (GraphQL playground, SDL emit) must see `test`. An explicit
        // value wins (e.g. CI asserting prod behaviour).
        if std::env::var_os("NESTRS_ENV").is_none() {
            // SAFETY: runs during single-threaded harness bootstrap, before
            // any app task or transport spawns — no concurrent env reader
            // exists. Test-harness env setup on the (non-test) lib build: the
            // sole sanctioned unsafe.
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var("NESTRS_ENV", "test")
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
