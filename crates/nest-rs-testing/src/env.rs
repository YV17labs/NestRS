//! Load the project's `.env` cascade for e2e: the harness reads backend URLs
//! before any `ConfigModule` exists, from a crate dir rather than the root.

use std::path::{Path, PathBuf};
use std::sync::Once;

use nest_rs_config::__private::load_app_cascade;
use nest_rs_config::Environment;

/// Load the project's `.env` cascade once per process, set-if-absent and
/// bounded to the git repo, defaulting `<PREFIX>_ENV=test` first: the files of
/// the package's Cargo workspace root — where `cargo run` and `nestrs run`
/// start its binary — under the package's own, the app's layer, as
/// `#[nest_rs::main]` reads them.
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
            // The write is what the console's settings and `OpenTelemetry::init` read
            // through bare `std::env::var`, which is the whole reason a
            // resolution-only cascade is not enough. `load_app_cascade` below does
            // one further `unsafe set_var` per key under its own note — so
            // this is not the crate's only unsafe, only its first.
            #[expect(unsafe_code, reason = "set_var before any thread reads the environment, per the SAFETY note above")]
            unsafe {
                std::env::set_var(&env_var, "test")
            };
        }
        let Ok(package) = std::env::current_dir() else {
            return;
        };
        load_app_cascade(&project_root(&package), &package, Environment::from_env());
    });
}

/// The directory `package`'s binary starts in: the nearest at or above it
/// whose manifest declares a Cargo workspace, short of a repository's root, or
/// `package` itself.
fn project_root(package: &Path) -> PathBuf {
    for dir in package.ancestors() {
        if declares_workspace(&dir.join("Cargo.toml")) {
            return dir.to_path_buf();
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    package.to_path_buf()
}

/// Whether `manifest` holds a `[workspace]` table, as cargo reads it.
fn declares_workspace(manifest: &Path) -> bool {
    std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .is_some_and(|manifest| manifest.contains_key("workspace"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("a file's directory")).expect("a directory");
        std::fs::write(path, contents).expect("a file");
    }

    #[test]
    fn the_root_is_the_package_s_workspace_and_the_package_is_its_layer() {
        let repo = tempfile::tempdir().expect("a scratch repository");
        let root = repo.path();
        std::fs::create_dir_all(root.join(".git")).expect("the repository's root");
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nexclude = [\"demo\"]\n",
        );
        write(
            &root.join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\n",
        );
        write(
            &root.join("demo/Cargo.toml"),
            "[workspace]\nmembers = [\"apps/*\"]\n",
        );
        write(&root.join("demo/.env"), "");
        write(
            &root.join("demo/apps/api/Cargo.toml"),
            "[package]\nname = \"api\"\n",
        );
        write(&root.join("demo/apps/api/.env"), "");
        write(
            &root.join("demo/crates/features/Cargo.toml"),
            "[package]\nname = \"features\"\n",
        );
        write(&root.join("standalone/.git/HEAD"), "");
        write(
            &root.join("standalone/Cargo.toml"),
            "[package]\nname = \"standalone\"\n",
        );

        let demo = root.join("demo");
        assert_eq!(
            project_root(&root.join("demo/apps/api")),
            demo,
            "an app's own `.env` is its layer, not the root"
        );
        assert_eq!(project_root(&root.join("demo/crates/features")), demo);
        assert_eq!(
            project_root(&root.join("crates/core")),
            root,
            "the nearest workspace, with no `.env` of its own, is still where the binary starts",
        );
        let standalone = root.join("standalone");
        assert_eq!(
            project_root(&standalone),
            standalone,
            "a repository of its own is its own root",
        );
    }

    #[test]
    fn a_package_of_its_own_below_another_env_is_its_own_root() {
        let repo = tempfile::tempdir().expect("a scratch repository");
        let root = repo.path();
        let backend = root.join("backend");
        std::fs::create_dir_all(root.join(".git")).expect("the repository's root");
        write(
            &backend.join("Cargo.toml"),
            "[package]\nname = \"backend\"\n",
        );
        write(&root.join(".env"), "");
        write(&backend.join(".env"), "");

        assert_eq!(
            project_root(&backend),
            backend,
            "`cargo run` starts the binary in its package, which never reads `repo/.env`",
        );
    }
}
