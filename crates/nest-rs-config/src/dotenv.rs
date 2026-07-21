//! Minimal `.env` cascade loader (dotenv-flow / Next.js precedence):
//!
//! ```text
//! real env  >  .env.<env>.local  >  .env.local  >  .env.<env>  >  .env
//! ```
//!
//! `.env.local` is skipped under [`Environment::Test`] so tests stay hermetic.
//!
//! The cascade is parsed once into an in-crate map (`dotenv_values`) that the
//! config layer consults through `env_var` — the real process env always wins,
//! dotenv only fills what the real env leaves unset. Resolving config therefore
//! **never mutates the process environment**, so no `set_var` can race a
//! concurrent `getenv` on a worker thread (`std::env::set_var` is `unsafe` and
//! unsound under that race). The one path that still writes to the process env
//! is `load_cascade` — an explicit, opt-in bootstrapper for callers that must
//! expose dotenv values via raw `std::env::var` before any `ConfigService`
//! exists (the e2e harness); the framework's live-runtime path never uses it.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use crate::environment::Environment;

/// The parsed `.env` cascade for the active [`Environment`], rooted at the
/// current directory. Built once, lazily; side-effect-free (reads files only —
/// **no** process-env mutation). Consulted by `env_var` as the fallback under
/// the real process environment, so config lookups see dotenv values without
/// `set_var`.
pub(crate) fn dotenv_values() -> &'static HashMap<String, String> {
    static VALUES: OnceLock<HashMap<String, String>> = OnceLock::new();
    VALUES.get_or_init(|| cascade_map(Path::new("."), Environment::from_env()))
}

/// Parse the `.env` cascade rooted at `dir` into a map (most-specific file
/// wins). Pure: reads files only, never touches the process environment and
/// never consults the real env — real-env precedence is applied at read time by
/// `env_var`.
pub(crate) fn cascade_map(dir: &Path, env: Environment) -> HashMap<String, String> {
    let e = env.as_str();
    // Most specific first: `or_insert` makes the first writer win, so this
    // order encodes the documented precedence.
    let mut files = vec![format!(".env.{e}.local")];
    if env != Environment::Test {
        files.push(".env.local".to_owned());
    }
    files.push(format!(".env.{e}"));
    files.push(".env".to_owned());

    let mut values = HashMap::new();
    for file in files {
        merge_file(&dir.join(file), &mut values);
    }
    values
}

/// Merge one `.env` file's assignments into `values` (set-if-absent — the
/// first writer, i.e. the most specific file, wins).
fn merge_file(path: &Path, values: &mut HashMap<String, String>) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        // A missing cascade file is the normal case — the loader walks every
        // candidate. Anything else (permissions, invalid UTF-8) is a present
        // file we failed to read: surface it rather than silently dropping it.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                target: "nest_rs::config",
                path = %path.display(),
                error = %e,
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
        let line = line.strip_prefix("export ").unwrap_or(line);
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
    // One aggregate warning per file — a malformed line means a config value the
    // user expects is quietly absent.
    if skipped > 0 {
        tracing::warn!(
            target: "nest_rs::config",
            path = %path.display(),
            skipped,
            "skipped malformed .env lines",
        );
    }
}

/// Merge the `.env` cascade rooted at `dir` into the **process environment**
/// (set-if-absent — real env wins). This is the only path that mutates the
/// process env; it exists for bootstrappers that read dotenv values via raw
/// `std::env::var` before an `App` (hence a `ConfigService`) exists — the e2e
/// harness. In-process, the config layer never calls this: it reads
/// `dotenv_values` through `env_var`, so a running app mutates nothing.
pub fn load_cascade(dir: &Path, env: Environment) {
    for (key, value) in cascade_map(dir, env) {
        if std::env::var_os(&key).is_some() {
            continue;
        }
        // SAFETY: `set_var` is unsound only when it races a concurrent `getenv`
        // on another thread. This is NOT the live-runtime path — the
        // framework's config reads go through `dotenv_values`/`env_var` and
        // never reach here. The only in-repo caller is an explicit bootstrapper
        // (the e2e harness) that invokes it during single-threaded setup,
        // before spawning any task that reads the environment; the write
        // therefore happens-before every later `getenv`. Any caller of this
        // public function carries that same obligation.
        // Sanctioned bootstrapper: the framework's one production env write.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var(&key, value)
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

    // `parse_value` is the per-line parser shared by every cascade tier — pin
    // each quoting variant directly so a future rewrite of `merge_file`
    // doesn't silently change PEM-multiline support.

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
        // \n / \t / \r / \\ / \" — the documented set; a PEM private key
        // ships as one logical line with \n for newlines, so this is
        // load-bearing.
        assert_eq!(parse_value(r#""a\nb""#), "a\nb");
        assert_eq!(parse_value(r#""a\tb""#), "a\tb");
        assert_eq!(parse_value(r#""a\rb""#), "a\rb");
        assert_eq!(parse_value(r#""a\\b""#), "a\\b");
        assert_eq!(parse_value(r#""quoted \"x\"""#), r#"quoted "x""#);
    }

    #[test]
    fn parse_value_double_quoted_preserves_unknown_escapes_verbatim() {
        // A `\z` isn't a known escape — keep the backslash + the char so the
        // user sees the typo rather than getting silent data loss.
        assert_eq!(parse_value(r#""a\zb""#), r"a\zb");
    }

    #[test]
    fn parse_value_double_quoted_trailing_backslash_is_kept_literal() {
        // Inner = `x\` (one literal backslash at end, no follower); keep it
        // verbatim instead of consuming the closing quote.
        let input = "\"x\\\""; // string `"x\"`
        assert_eq!(parse_value(input), "x\\"); // string `x\`
    }

    #[test]
    fn parse_value_single_quoted_is_literal_no_escape_expansion() {
        // Single quotes are the literal form — `\n` stays two chars.
        assert_eq!(parse_value(r#"'a\nb'"#), r"a\nb");
        assert_eq!(parse_value("'plain'"), "plain");
    }

    #[test]
    fn parse_value_mismatched_quotes_are_not_treated_as_quoted() {
        // `"x'` — different opening/closing quote chars: not quoted.
        assert_eq!(parse_value(r#""x'"#), r#""x'"#);
        // Single char that looks like an opening quote: not quoted.
        assert_eq!(parse_value(r#"""#), r#"""#);
    }

    // `merge_file` exercises every parse-line branch (comment, empty,
    // missing equal, `export` prefix, set-if-absent). Drive them all via
    // a temp file.

    #[test]
    #[allow(clippy::result_large_err)]
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
    #[allow(clippy::result_large_err)]
    fn merge_file_skips_empty_key_and_lines_with_only_whitespace_key() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "=value-without-key\n   =whitespace-key\nVALID_KEY=ok\n",
            )?;
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("VALID_KEY").unwrap(), "ok");
            // The bad lines must not be loaded under any key.
            assert!(std::env::var("").is_err());
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn merge_file_is_a_no_op_when_the_path_doesnt_exist() {
        figment::Jail::expect_with(|jail| {
            // No `.env` files created — load_cascade walks all candidates and
            // every read fails silently. We mainly check that no panic
            // happens and existing env stays intact.
            jail.set_env("CASCADE_PRESERVE_ME", "kept");
            load_cascade(Path::new("."), Environment::Development);
            assert_eq!(std::env::var("CASCADE_PRESERVE_ME").unwrap(), "kept");
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
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

    // `cascade_map` is the live-runtime path: it resolves file precedence into a
    // map **without** mutating the process env. Pin both — most-specific file
    // wins, and no `set_var` leaks the values into `std::env` (the whole point
    // of the fix — the config layer reads this map, it does not merge it).
    #[test]
    #[allow(clippy::result_large_err)]
    fn cascade_map_resolves_precedence_without_touching_process_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(".env", "MAP_A=base\nMAP_B=base")?;
            jail.create_file(".env.development", "MAP_B=dev")?;
            jail.create_file(".env.development.local", "MAP_B=dev_local")?;
            let map = cascade_map(Path::new("."), Environment::Development);
            assert_eq!(map.get("MAP_A").map(String::as_str), Some("base"));
            assert_eq!(map.get("MAP_B").map(String::as_str), Some("dev_local"));
            // The read path must not have written anything into the real env.
            assert!(std::env::var("MAP_A").is_err());
            assert!(std::env::var("MAP_B").is_err());
            Ok(())
        });
    }
}
