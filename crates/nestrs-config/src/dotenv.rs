//! A minimal `.env` file loader — the per-environment cascade behind
//! [`ConfigModule::for_root`](crate::ConfigModule::for_root).
//!
//! Hand-rolled (no dependency) and deliberately tiny: it reads `KEY=VALUE` lines
//! and sets each variable **only if it is not already set**, so the real process
//! environment always wins over any file. Files are loaded most-specific first,
//! so an earlier (more specific) file wins over a later one — yielding the
//! precedence:
//!
//! ```text
//! real env  >  .env.<env>.local  >  .env.local  >  .env.<env>  >  .env
//! ```
//!
//! `.env.local` is skipped under [`Environment::Test`] so tests stay hermetic.
//! This is the dotenv-flow / Next.js convention, the one most developers already
//! know. Loading is a dev/local convenience; production sets real env vars and
//! ships no files.

use std::fs;
use std::path::Path;
use std::sync::Once;

use crate::environment::Environment;

/// Load the `.env` cascade **once per process**, from the working directory, for
/// the active [`Environment`]. The single choke point every config read routes
/// through (`load_namespaced` / `load` call it, as do `ConfigModule::for_root` and
/// `bootstrap_env`), so the cascade is loaded even if an app forgets to wire any
/// of them — and never loaded twice. Real env vars still win (set-if-absent).
pub(crate) fn ensure_env_loaded() {
    static LOADED: Once = Once::new();
    LOADED.call_once(|| {
        load_cascade(Path::new("."), Environment::from_env());
    });
}

/// Load the `.env` cascade for `env` from `dir`, filling only variables that are
/// not already present in the process environment.
pub(crate) fn load_cascade(dir: &Path, env: Environment) {
    let e = env.as_str();
    // Most specific first: with the set-if-absent rule below, the first file to
    // define a key wins, so this order encodes the documented precedence.
    let mut files = vec![format!(".env.{e}.local")];
    if env != Environment::Test {
        files.push(".env.local".to_owned());
    }
    files.push(format!(".env.{e}"));
    files.push(".env".to_owned());

    for file in files {
        load_file(&dir.join(file));
    }
}

/// Parse one `.env` file (ignored if absent), setting each `KEY=VALUE` whose key
/// is not already set in the environment.
fn load_file(path: &Path) {
    let Ok(contents) = fs::read_to_string(path) else {
        return; // a missing file is normal — the cascade is best-effort.
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Optional `export ` prefix, like a shell-sourced file.
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue; // real env (or an earlier, more specific file) wins.
        }
        std::env::set_var(key, parse_value(value.trim()));
    }
}

/// Interpret a value the standard dotenv way: a **double-quoted** value has its
/// quotes stripped and `\n` / `\t` / `\r` / `\\` / `\"` escapes expanded (so a
/// PEM key can live on one line as `"…\n…"`); a **single-quoted** value is
/// literal (quotes stripped, no expansion); an unquoted value is taken as-is.
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
        return inner.to_owned(); // single quotes: literal, no escapes.
    }
    // Double quotes: expand the common escapes.
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
mod tests {
    use super::*;

    // `figment::Jail` gives a temp cwd + an env lock. `load_cascade` writes through
    // the **global** process env (`set_var`), and the set-if-absent rule keys off
    // it, so each test uses a **unique variable name** to stay independent of any
    // other test's writes within the shared process. The `result_large_err` allow
    // matches the other config tests: `Jail`'s closure returns figment's large
    // `Result`, a signature we cannot change.

    #[test]
    #[allow(clippy::result_large_err)]
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
    #[allow(clippy::result_large_err)]
    fn most_specific_file_wins() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_B=base")?;
            jail.create_file(".env.development", "CASCADE_B=dev")?;
            jail.create_file(".env.local", "CASCADE_B=local")?;
            jail.create_file(".env.development.local", "CASCADE_B=dev_local")?;
            // Order: .env.development.local > .env.local > .env.development > .env
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("CASCADE_B").unwrap(), "dev_local");
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn local_overrides_env_specific_which_overrides_base() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_C=base\nCASCADE_D=base\nCASCADE_E=base")?;
            jail.create_file(".env.development", "CASCADE_D=dev\nCASCADE_E=dev")?;
            jail.create_file(".env.local", "CASCADE_E=local")?;
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("CASCADE_C").unwrap(), "base"); // only in .env
            assert_eq!(std::env::var("CASCADE_D").unwrap(), "dev"); // .env.development > .env
            assert_eq!(std::env::var("CASCADE_E").unwrap(), "local"); // .env.local wins
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn test_environment_skips_env_local_for_hermeticity() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "CASCADE_F=base")?;
            jail.create_file(".env.local", "CASCADE_F=local_secret")?;
            jail.create_file(".env.test", "CASCADE_F=test")?;
            // `.env.local` must NOT load under Test, so the committed `.env.test`
            // (then `.env`) wins — tests stay hermetic.
            load_cascade(Path::new("."), Environment::Test);
            assert_eq!(std::env::var("CASCADE_F").unwrap(), "test");
            Ok(())
        });
    }
}
