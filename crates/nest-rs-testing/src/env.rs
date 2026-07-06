//! Load the project's `.env` cascade for e2e — the harness reads backend URLs
//! via `std::env::var` before any `App` (hence `ConfigModule`) exists, and
//! tests run from a crate dir, not the project root that holds `.env`.

use std::sync::Once;

use nest_rs_config::{Environment, load_cascade};

/// Load the nearest project `.env` once per process. Set-if-absent (real env /
/// CI wins); bounded to the git repo so the framework's own `.env`-less tests
/// stay hermetic.
pub fn load_project_env() {
    static LOADED: Once = Once::new();
    LOADED.call_once(|| {
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
