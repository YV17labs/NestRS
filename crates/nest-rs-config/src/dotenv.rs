//! Minimal `.env` cascade loader (dotenv-flow / Next.js precedence):
//!
//! ```text
//! real env  >  .env.<env>.local  >  .env.local  >  .env.<env>  >  .env
//! ```
//!
//! `.env.local` is skipped under [`Environment::Test`] so tests stay hermetic.
//!
//! Under a declared development or test profile, the app's own directory — the
//! crate `#[nest_rs::main]` expanded in, `apps/<bin>/` in a workspace — holds a
//! layer of the same four files, read whole over the root's (Nx's order): the
//! one place a value that identifies one binary of a workspace is written for
//! it alone. A deployed profile reads the root alone.
//!
//! The cascade is parsed once into an in-crate map that `env_var` consults under
//! the real process env, so resolving config **never mutates the process
//! environment** (`set_var` is unsound against a concurrent `getenv`). Two
//! callers write it, each while the process has one thread: the step
//! `#[nest_rs::main]` runs before the runtime exists, and `load_cascade`, for a
//! binary without it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, OnceLock, PoisonError, RwLock};

use nest_rs_core::EnvPrefix;

use crate::environment::Environment;

/// The parsed `.env` cascade for the active [`Environment`], rooted at the
/// current directory under the app's own layer. Built once, lazily, without
/// mutating the process env.
pub(crate) fn dotenv_values() -> &'static HashMap<String, String> {
    static VALUES: OnceLock<HashMap<String, String>> = OnceLock::new();
    VALUES.get_or_init(|| {
        cascade_map(
            Path::new("."),
            nest_rs_core::__private::app_dir(),
            Environment::from_env(),
        )
    })
}

/// Parse the `.env` cascade rooted at `root` into a map (most-specific file
/// wins), under `app`'s own files when the profile reads them, without
/// touching the process environment.
pub(crate) fn cascade_map(
    root: &Path,
    app: Option<&Path>,
    env: Environment,
) -> HashMap<String, String> {
    let e = env.as_str();
    // Most specific first: `or_insert` makes the first writer win.
    let mut files = vec![format!(".env.{e}.local")];
    if env != Environment::Test {
        files.push(".env.local".to_owned());
    }
    files.push(format!(".env.{e}"));
    files.push(".env".to_owned());

    let mut values = HashMap::new();
    for dir in app_layer(root, app, env).into_iter().chain([root]) {
        for file in &files {
            merge_file(&dir.join(file), &mut values);
        }
    }
    assert_prefix_not_from_cascade(&values);
    assert_environment_not_from_cascade(&values);
    values
}

/// `app`, when its files form a layer of their own: under a development or
/// test profile only — a deployment's binary runs far from the directory it
/// was built in — and never the root itself, read once.
fn app_layer<'a>(root: &Path, app: Option<&'a Path>, env: Environment) -> Option<&'a Path> {
    let app = app.filter(|_| !env.is_deployed())?;
    let same = matches!(
        (app.canonicalize(), root.canonicalize()),
        (Ok(app), Ok(root)) if app == root
    );
    (!same).then_some(app)
}

/// Abort if the cascade tries to name the env prefix: the prefix chose the
/// cascade before it was read.
fn assert_prefix_not_from_cascade(values: &HashMap<String, String>) {
    let Some(declared) = values.get(EnvPrefix::VAR) else {
        return;
    };
    let resolved = EnvPrefix::current();
    assert!(
        declared == resolved,
        "{} is set in the `.env` cascade to another prefix than the one already resolved, \
         `{resolved}`. The prefix chooses which cascade to read, so a value inside it arrives \
         too late to have done so — set it on the process instead (your container, your \
         shell, the Justfile).",
        EnvPrefix::VAR,
    );
}

/// Abort if the cascade tries to name the active environment.
///
/// Stricter than the prefix: the file may only restate the value the
/// **process** carries, since [`publish`] would otherwise launder it into
/// `Environment::declared` and arm development-only affordances.
fn assert_environment_not_from_cascade(values: &HashMap<String, String>) {
    let var = Environment::var_name();
    let Some(declared) = values.get(&var) else {
        return;
    };
    let process = crate::source::real_env_var(&var);
    assert!(
        process.as_deref() == Some(declared.as_str()),
        "{var} is set in the `.env` cascade, but the process environment {}. \
         The variable chooses which `.env` files to read, so a value inside one arrives too \
         late to have done so — and it arms development-only affordances, which must never \
         answer to a committed file. Set it on the process instead (`nestrs run dev` does) \
         and remove it from the file.",
        if process.is_some() {
            "carries another value"
        } else {
            "does not carry it"
        },
    );
}

/// Merge one `.env` file's assignments into `values` (set-if-absent — the
/// first writer, i.e. the most specific file, wins).
fn merge_file(path: &Path, values: &mut HashMap<String, String>) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                target: crate::TARGET,
                path = %path.display(),
                error = %nest_rs_core::error_message(&e),
                "skipping unreadable .env file",
            );
            return;
        }
    };
    let mut skipped = 0usize;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.trim_prefix("export ");
        let Some((key, value)) = line.split_once('=') else {
            skipped += 1;
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            skipped += 1;
            continue;
        }
        values
            .entry(key.to_owned())
            .or_insert_with(|| parse_value(value.trim()));
    }
    if skipped > 0 {
        tracing::warn!(
            target: crate::TARGET,
            path = %path.display(),
            skipped,
            "skipped malformed .env lines",
        );
    }
}

/// Merge the `.env` cascade rooted at `dir` into the **process environment**
/// (set-if-absent — real env wins), for consumers reading raw `std::env::var`.
///
/// `#[nest_rs::main]` already does this before the runtime exists; a binary
/// without it calls this first thing.
///
/// Call it only single-threaded, before any task reads the environment: it
/// runs `std::env::set_var`.
///
/// ```no_run
/// use nest_rs_config::{Environment, load_cascade};
///
/// fn main() {
///     load_cascade(std::path::Path::new("."), Environment::from_env());
///     // Only now start a runtime, or any other thread.
/// }
/// ```
pub fn load_cascade(dir: &Path, env: Environment) {
    publish(&cascade_map(dir, None, env));
}

/// [`load_cascade`] for the app whose own directory is `app`, its files read
/// over `root`'s under a development or test profile: what the test harness
/// runs for the package under test.
pub fn load_app_cascade(root: &Path, app: &Path, env: Environment) {
    publish(&cascade_map(root, Some(app), env));
}

/// Publish the cascade already parsed into the in-crate map.
fn publish_dotenv_values() {
    publish(dotenv_values());
}

// Runs from `#[nest_rs::main]` before the runtime is built, which is the
// precondition `publish`'s `set_var` rests on.
nest_rs_core::inventory::submit! {
    nest_rs_core::__private::ProcessStart {
        name: "nest_rs::config::cascade",
        run: publish_dotenv_values,
    }
}

/// Names this process wrote into `std::env` **from a cascade file** — the real
/// environment left them unset and a committed `.env` supplied the value.
///
/// Once published, `std::env::var` cannot tell a deployment variable from a
/// committed file; these names restore it for
/// [`ConfigSource::get_from_deployment`].
///
/// [`ConfigSource::get_from_deployment`]: crate::ConfigSource::get_from_deployment
static PUBLISHED: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(|| RwLock::new(HashSet::new()));

/// Whether `name`'s current `std::env` value came from a cascade file rather
/// than from the deployment. Read at boot by the config layer only.
pub(crate) fn published_from_cascade(name: &str) -> bool {
    PUBLISHED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(name)
}

/// Set-if-absent, the single process-env write.
#[expect(
    clippy::disallowed_methods,
    reason = "the cascade is what fills the process environment; it reads it to write set-if-absent"
)]
fn publish(values: &HashMap<String, String>) {
    let mut published = PUBLISHED.write().unwrap_or_else(PoisonError::into_inner);
    for (key, value) in values {
        if std::env::var_os(key).is_some() {
            continue;
        }
        published.insert(key.clone());
        // SAFETY: `set_var` is unsound only when it races a concurrent `getenv`.
        // Config resolution never reaches here. The process step runs from
        // `#[nest_rs::main]` before the runtime is built, so no other thread
        // exists in a binary; `load_cascade`'s caller promises the same.
        #[expect(
            unsafe_code,
            reason = "set_var before any thread reads the environment, per the SAFETY note above"
        )]
        unsafe {
            std::env::set_var(key, value)
        };
    }
}

/// Double-quoted: expand `\n \t \r \\ \"` so a PEM key fits on one line.
/// Single-quoted: literal. Unquoted: as-is.
fn parse_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let quoted = bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0];
    if !quoted {
        return value.to_owned();
    }
    let inner = &value[1..value.len() - 1];
    if bytes[0] == b'\'' {
        return inner.to_owned();
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "the cascade's contract is the process environment it writes, so the tests read it"
)]
mod tests {
    use super::*;

    // Each test uses a unique variable name: load_cascade writes the global
    // process env via set_var, and set-if-absent keys off it.

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn real_env_wins_over_every_file() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_A=from_base")?;
            jail.create_file(".env.development", "CASCADE_A=from_dev")?;
            jail.create_file(".env.development.local", "CASCADE_A=from_dev_local")?;
            jail.set_env("CASCADE_A", "from_real_env");
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("CASCADE_A").unwrap(), "from_real_env");
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn most_specific_file_wins() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_B=base")?;
            jail.create_file(".env.development", "CASCADE_B=dev")?;
            jail.create_file(".env.local", "CASCADE_B=local")?;
            jail.create_file(".env.development.local", "CASCADE_B=dev_local")?;
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("CASCADE_B").unwrap(), "dev_local");
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn local_overrides_env_specific_which_overrides_base() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_C=base\nCASCADE_D=base\nCASCADE_E=base")?;
            jail.create_file(".env.development", "CASCADE_D=dev\nCASCADE_E=dev")?;
            jail.create_file(".env.local", "CASCADE_E=local")?;
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("CASCADE_C").unwrap(), "base");
            assert_eq!(std::env::var("CASCADE_D").unwrap(), "dev");
            assert_eq!(std::env::var("CASCADE_E").unwrap(), "local");
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn test_environment_skips_env_local_for_hermeticity() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_F=base")?;
            jail.create_file(".env.local", "CASCADE_F=local_secret")?;
            jail.create_file(".env.test", "CASCADE_F=test")?;
            load_cascade(Path::new("."), Environment::Test);
            assert_eq!(std::env::var("CASCADE_F").unwrap(), "test");
            Ok(())
        });
    }

    #[test]
    fn parse_value_unquoted_passes_through_unchanged() {
        assert_eq!(parse_value("plain"), "plain");
        assert_eq!(parse_value("with spaces"), "with spaces");
        assert_eq!(parse_value(""), "");
    }

    #[test]
    fn parse_value_double_quoted_strips_quotes() {
        assert_eq!(parse_value(r#""hello""#), "hello");
    }

    #[test]
    fn parse_value_double_quoted_expands_escapes() {
        assert_eq!(parse_value(r#""a\nb""#), "a\nb");
        assert_eq!(parse_value(r#""a\tb""#), "a\tb");
        assert_eq!(parse_value(r#""a\rb""#), "a\rb");
        assert_eq!(parse_value(r#""a\\b""#), "a\\b");
        assert_eq!(parse_value(r#""quoted \"x\"""#), r#"quoted "x""#);
    }

    #[test]
    fn parse_value_double_quoted_preserves_unknown_escapes_verbatim() {
        assert_eq!(parse_value(r#""a\zb""#), r"a\zb");
    }

    #[test]
    fn parse_value_double_quoted_trailing_backslash_is_kept_literal() {
        let input = "\"x\\\""; // string `"x\"`
        assert_eq!(parse_value(input), "x\\"); // string `x\`
    }

    #[test]
    fn parse_value_single_quoted_is_literal_no_escape_expansion() {
        assert_eq!(parse_value(r#"'a\nb'"#), r"a\nb");
        assert_eq!(parse_value("'plain'"), "plain");
    }

    #[test]
    fn parse_value_mismatched_quotes_are_not_treated_as_quoted() {
        assert_eq!(parse_value(r#""x'"#), r#""x'"#);
        assert_eq!(parse_value(r#"""#), r#"""#);
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn merge_file_handles_comments_blank_lines_and_export_prefix() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "# a comment\n\nLOAD_A=one\nexport LOAD_B=two\nno-equals-here\nLOAD_C=three\n",
            )?;
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("LOAD_A").unwrap(), "one");
            assert_eq!(std::env::var("LOAD_B").unwrap(), "two");
            assert_eq!(std::env::var("LOAD_C").unwrap(), "three");
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn merge_file_skips_empty_key_and_lines_with_only_whitespace_key() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "=value-without-key\n   =whitespace-key\nVALID_KEY=ok\n",
            )?;
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("VALID_KEY").unwrap(), "ok");
            assert!(std::env::var("").is_err());
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn merge_file_is_a_no_op_when_the_path_doesnt_exist() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("CASCADE_PRESERVE_ME", "kept");
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("CASCADE_PRESERVE_ME").unwrap(), "kept");
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn merge_file_expands_double_quoted_pem_style_value() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "PEM_KEY=\"-----BEGIN-----\\nMIIB\\n-----END-----\"\n",
            )?;
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(
                std::env::var("PEM_KEY").unwrap(),
                "-----BEGIN-----\nMIIB\n-----END-----",
            );
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn publish_marks_only_the_keys_the_cascade_actually_supplied() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "PUB_FROM_FILE=file\nPUB_FROM_REAL=file")?;
            jail.set_env("PUB_FROM_REAL", "real");
            load_cascade(Path::new("."), Environment::Development);

            assert!(
                published_from_cascade("PUB_FROM_FILE"),
                "a committed file supplied this value, so it is not a deployment variable",
            );
            assert!(
                !published_from_cascade("PUB_FROM_REAL"),
                "set-if-absent skipped this key — the deployment still owns it",
            );
            assert!(!published_from_cascade("PUB_NEVER_SEEN"));
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn a_poisoned_publication_record_still_says_what_the_cascade_supplied() {
        crate::testing::panic_holding(|| PUBLISHED.write());
        assert!(PUBLISHED.is_poisoned());
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "POISONED_PUB_FROM_FILE=file")?;
            load_cascade(Path::new("."), Environment::Development);
            assert!(
                published_from_cascade("POISONED_PUB_FROM_FILE"),
                "a committed file supplied this value, poisoned record or not",
            );
            Ok(())
        });
    }

    /// The app's four files, then the root's: a value that identifies one
    /// binary of a workspace is written in its own directory alone.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn an_app_layer_is_read_whole_over_the_root_under_development() {
        figment::Jail::expect_with(|jail| {
            jail.create_dir("apps/api")?;
            jail.create_file(".env", "LAYER_A=root\nLAYER_B=root\nLAYER_C=root")?;
            jail.create_file(".env.local", "LAYER_C=root_local")?;
            jail.create_file(".env.development", "LAYER_D=root_dev")?;
            jail.create_file("apps/api/.env", "LAYER_A=app\nLAYER_C=app\nLAYER_D=app")?;
            let map = cascade_map(
                Path::new("."),
                Some(Path::new("apps/api")),
                Environment::Development,
            );
            assert_eq!(map.get("LAYER_A").map(String::as_str), Some("app"));
            assert_eq!(map.get("LAYER_B").map(String::as_str), Some("root"));
            assert_eq!(
                map.get("LAYER_C").map(String::as_str),
                Some("app"),
                "the app's committed file outranks every file of the root, its `.local` included",
            );
            assert_eq!(map.get("LAYER_D").map(String::as_str), Some("app"));
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn the_test_profile_reads_the_app_layer_without_its_local_file() {
        figment::Jail::expect_with(|jail| {
            jail.create_dir("apps/api")?;
            jail.create_file(".env", "LAYER_E=root\nLAYER_F=root")?;
            jail.create_file("apps/api/.env", "LAYER_E=app")?;
            jail.create_file("apps/api/.env.local", "LAYER_F=app_local")?;
            let map = cascade_map(
                Path::new("."),
                Some(Path::new("apps/api")),
                Environment::Test,
            );
            assert_eq!(map.get("LAYER_E").map(String::as_str), Some("app"));
            assert_eq!(
                map.get("LAYER_F").map(String::as_str),
                Some("root"),
                "`.env.local` stays unread under test, in the app's layer as in the root's",
            );
            Ok(())
        });
    }

    /// A deployment's binary runs far from the directory it was built in: its
    /// own files are a development convenience, never a deployed one's input.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn a_deployed_profile_reads_no_app_layer() {
        figment::Jail::expect_with(|jail| {
            jail.create_dir("apps/api")?;
            jail.create_file(".env", "LAYER_G=root")?;
            jail.create_file("apps/api/.env", "LAYER_G=app\nLAYER_H=app")?;
            for env in [Environment::Production, Environment::Staging] {
                let map = cascade_map(Path::new("."), Some(Path::new("apps/api")), env);
                assert_eq!(
                    map.get("LAYER_G").map(String::as_str),
                    Some("root"),
                    "{env:?}"
                );
                assert_eq!(map.get("LAYER_H"), None, "{env:?}");
            }
            Ok(())
        });
    }

    /// The app's files choose no cascade either: they are read after the
    /// selector picked them.
    #[test]
    #[should_panic(expected = "is set in the `.env` cascade")]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn the_environment_written_into_the_app_layer_aborts() {
        figment::Jail::expect_with(|jail| {
            jail.create_dir("apps/api")?;
            jail.create_file(
                "apps/api/.env",
                &format!("{}=development", Environment::var_name()),
            )?;
            cascade_map(
                Path::new("."),
                Some(Path::new("apps/api")),
                Environment::Development,
            );
            Ok(())
        });
    }

    /// The harness's entry: the package's own files reach the process
    /// environment over the root's, and are marked as the cascade's.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn load_app_cascade_publishes_the_app_layer_over_the_root() {
        figment::Jail::expect_with(|jail| {
            jail.create_dir("apps/api")?;
            jail.create_file(".env", "LAYER_I=root\nLAYER_J=root")?;
            jail.create_file("apps/api/.env", "LAYER_I=app")?;
            load_app_cascade(Path::new("."), Path::new("apps/api"), Environment::Test);
            assert_eq!(std::env::var("LAYER_I").unwrap(), "app");
            assert_eq!(std::env::var("LAYER_J").unwrap(), "root");
            assert!(published_from_cascade("LAYER_I"));
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn cascade_map_resolves_precedence_without_touching_process_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "MAP_A=base\nMAP_B=base")?;
            jail.create_file(".env.development", "MAP_B=dev")?;
            jail.create_file(".env.development.local", "MAP_B=dev_local")?;
            let map = cascade_map(Path::new("."), None, Environment::Development);
            assert_eq!(map.get("MAP_A").map(String::as_str), Some("base"));
            assert_eq!(map.get("MAP_B").map(String::as_str), Some("dev_local"));
            assert!(std::env::var("MAP_A").is_err());
            assert!(std::env::var("MAP_B").is_err());
            Ok(())
        });
    }
}
