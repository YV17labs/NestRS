//! Minimal `.env` cascade loader (dotenv-flow / Next.js precedence):
//!
//! ```text
//! real env  >  .env.<env>.local  >  .env.local  >  .env.<env>  >  .env
//! ```
//!
//! `.env.local` is skipped under [`Environment::Test`] so tests stay hermetic.
//! Set-if-absent — real env always wins; load is best-effort.

use std::fs;
use std::path::Path;
use std::sync::Once;

use crate::environment::Environment;

/// Once-per-process choke point all config reads route through.
pub(crate) fn ensure_env_loaded() {
    static LOADED: Once = Once::new();
    LOADED.call_once(|| {
        load_cascade(Path::new("."), Environment::from_env());
    });
}

pub(crate) fn load_cascade(dir: &Path, env: Environment) {
    let e = env.as_str();
    // Most specific first: set-if-absent makes the first writer win, so this
    // order encodes the documented precedence.
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

fn load_file(path: &Path) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue;
        }
        std::env::set_var(key, parse_value(value.trim()));
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
mod tests {
    use super::*;

    // Each test uses a unique variable name: load_cascade writes the global
    // process env via set_var, and set-if-absent keys off it.

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
            assert_eq!(std::env::var("CASCADE_C").unwrap(), "base");
            assert_eq!(std::env::var("CASCADE_D").unwrap(), "dev");
            assert_eq!(std::env::var("CASCADE_E").unwrap(), "local");
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
            load_cascade(Path::new("."), Environment::Test);
            assert_eq!(std::env::var("CASCADE_F").unwrap(), "test");
            Ok(())
        });
    }
}
