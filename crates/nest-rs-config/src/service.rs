//! Framework env-var scheme `<PREFIX>_<DOMAIN>__<KEY>` and the typed
//! [`ConfigService`] reader handed to a config's `from_env`.
//!
//! A crate maps its own namespace: a read through this reader claims the
//! variable for the config in flight, and another config reading it raises
//! [`ConfigError::ContestedVariable`](crate::ConfigError). The free
//! [`env_var`](crate::env_var) arms no window and claims nothing.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use nest_rs_core::EnvPrefix;

use crate::error::ConfigError;
use crate::material::{Material, read_material};
use crate::setting::Setting;
use crate::source::{ConfigSource, EnvSource, MapSource};

thread_local! {
    /// Every variable name read while a `Config::resolve` is in flight; kept on
    /// the reader because a `from_env` hands it to sub-readers that are no `Config`.
    static READING: RefCell<Option<BTreeSet<String>>> = const { RefCell::new(None) };
}

/// Which config type owns each variable read so far in this process.
static CLAIMED: Mutex<BTreeMap<String, Owner>> = Mutex::new(BTreeMap::new());

/// A claiming config type: what decides, and what a message prints.
#[derive(Clone, Copy)]
struct Owner {
    id: std::any::TypeId,
    name: &'static str,
}

/// Record every variable `load` reads, and refuse a name another type already
/// claimed.
///
/// Records the resolved name, never the key: `("social__google", "CLIENT_ID")`
/// and `("social", "GOOGLE__CLIENT_ID")` are one variable. A namespace read with
/// no `Config` behind it (`nest-rs-opentelemetry`'s, before the container) is
/// not covered.
pub(crate) fn claiming<C: 'static, T>(load: impl FnOnce() -> T) -> (T, Result<(), ConfigError>) {
    /// Restores the outer window even if `from_env` unwinds.
    struct Window(Option<BTreeSet<String>>);
    impl Drop for Window {
        fn drop(&mut self) {
            READING.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }

    // `TypeId` decides: `type_name` is documented as not unique.
    let owner = Owner {
        id: std::any::TypeId::of::<C>(),
        name: std::any::type_name::<C>(),
    };
    let outer = Window(READING.with(|cell| cell.replace(Some(BTreeSet::new()))));
    let value = load();
    let read = READING.with(|cell| cell.take()).unwrap_or_default();
    drop(outer);

    // A poisoned bookkeeping mutex skips the check rather than failing the boot.
    let Ok(mut claimed) = CLAIMED.lock() else {
        return (value, Ok(()));
    };
    for var in read {
        match claimed.get(&var) {
            Some(existing) if existing.id != owner.id => {
                let err = ConfigError::ContestedVariable {
                    var,
                    owner: existing.name,
                    claimant: owner.name,
                };
                return (value, Err(err));
            }
            Some(_) => {}
            None => {
                claimed.insert(var, owner);
            }
        }
    }
    (value, Ok(()))
}

/// The fully-qualified name of a namespaced config variable:
/// `<PREFIX>_<DOMAIN>__<KEY>`.
///
/// For citing a variable with no reader in hand; a hardcoded name is wrong
/// under a custom prefix.
///
/// ```
/// use nest_rs_config::var_name;
/// use nest_rs_core::EnvPrefix;
///
/// // Built, not spelled — the assertion would be false the moment a deployment
/// // sets `NESTRS_ENV_PREFIX`, which is the one thing this function exists for.
/// assert_eq!(
///     var_name("seaorm", "URL"),
///     format!("{}_SEAORM__URL", EnvPrefix::current()),
/// );
/// ```
pub fn var_name(namespace: &str, key: &str) -> String {
    format!(
        "{}_{}__{}",
        EnvPrefix::current(),
        namespace.to_ascii_uppercase(),
        key.to_ascii_uppercase(),
    )
}

/// Both spellings of a namespaced variable, as a sentence names a setting the
/// deployment may give either way: `<PREFIX>_<DOMAIN>__<KEY> (or
/// <PREFIX>_<DOMAIN>__<KEY>_FILE)`.
///
/// For citing a variable that is not the one refused; a value that was read is
/// refused through [`Setting::refuse`], which names the spelling set.
///
/// ```
/// use nest_rs_config::{spellings, var_name};
///
/// assert_eq!(
///     spellings("storage", "SECRET_KEY"),
///     format!(
///         "{} (or {})",
///         var_name("storage", "SECRET_KEY"),
///         var_name("storage", "SECRET_KEY_FILE"),
///     ),
/// );
/// ```
pub fn spellings(namespace: &str, key: &str) -> String {
    format!(
        "{} (or {})",
        var_name(namespace, key),
        var_name(namespace, &file_key(key))
    )
}

/// Which tiers of the environment outrank the value a field falls back to:
/// `real env > pinned in code > .env cascade > in-code defaults`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Precedence {
    /// The base is the config's own defaults, so every tier outranks it.
    #[default]
    OverDefaults,
    /// The base is a value pinned at the call site (`Module::for_root(cfg)`), so
    /// only [`ConfigSource::get_from_deployment`] outranks it.
    OverPinned,
}

/// Typed reader bound to one namespace; resolves `<PREFIX>_<NAMESPACE>__<KEY>`.
pub struct ConfigService {
    namespace: String,
    source: Arc<dyn ConfigSource>,
    precedence: Precedence,
    /// Whether this reader answers from the deployment's environment, the one
    /// kind that triggers the unclaimed-variable report ([`crate::unclaimed`]).
    environment: bool,
}

impl ConfigService {
    /// A reader scoped to `namespace`, backed by the process/`.env` environment.
    pub fn for_namespace(namespace: &str) -> Self {
        Self {
            environment: true,
            ..Self::with_source(namespace, Arc::new(EnvSource))
        }
    }

    /// A reader backed by a custom [`ConfigSource`], the sole authority: the
    /// `.env` cascade is not merged.
    pub fn with_source(namespace: &str, source: Arc<dyn ConfigSource>) -> Self {
        Self {
            namespace: namespace.to_owned(),
            source,
            precedence: Precedence::OverDefaults,
            environment: false,
        }
    }

    /// Narrow this reader to the tiers that outrank a **code-pinned** value: the
    /// deployment still wins, the `.env` cascade defers to the pin.
    pub fn over_pinned(mut self) -> Self {
        self.precedence = Precedence::OverPinned;
        self
    }

    /// A hermetic reader backed by an in-memory map keyed by **`<KEY>` alone** —
    /// the string [`get`](Self::get) is asked for, never the full variable name.
    ///
    /// ```
    /// # use nest_rs_config::ConfigService;
    /// let cfg = ConfigService::with_vars("app", [("PORT", "8080")]);
    /// assert_eq!(cfg.get("PORT")?.as_deref(), Some("8080"));
    /// assert_eq!(cfg.get("MISSING")?, None);
    /// # Ok::<(), nest_rs_config::ConfigError>(())
    /// ```
    pub fn with_vars<'a>(
        namespace: &str,
        vars: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        let qualified: Vec<(String, String)> = vars
            .into_iter()
            .map(|(key, value)| (var_name(namespace, key), value.to_owned()))
            .collect();
        Self::with_source(namespace, Arc::new(MapSource::from_iter(qualified)))
    }

    /// The full `<PREFIX>_<NAMESPACE>__<KEY>` variable **name** (not its value)
    /// — for error messages and docs that must cite the exact variable.
    pub fn var_name(&self, key: &str) -> String {
        var_name(&self.namespace, key)
    }

    /// The namespace this reader resolves keys in, as it was given.
    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Whether this reader answers from the deployment's environment.
    pub(crate) fn reads_environment(&self) -> bool {
        self.environment
    }

    /// Both spellings of `key` in this namespace — see the free [`spellings`].
    pub fn spellings(&self, key: &str) -> String {
        spellings(&self.namespace, key)
    }

    /// The value of `key` in this namespace, from whichever of its two spellings
    /// the deployment set: inline in `<KEY>`, or in the file `<KEY>_FILE` names —
    /// the container-secrets convention, so a secret never has to sit in the
    /// process environment. `Ok(None)` when neither is set in any tier this
    /// reader's precedence lets through.
    ///
    /// A value read from a file must be UTF-8 and has its trailing line breaks
    /// removed; an empty file is unset. Both spellings set in one tier, or a file
    /// that cannot be read, is boot-fatal — see [`material`](Self::material).
    pub fn get(&self, key: &str) -> Result<Option<String>, ConfigError> {
        Ok(self.setting(key)?.map(|setting| setting.value))
    }

    /// [`get`](Self::get), keeping which spelling supplied the value — for a
    /// consumer that judges the value itself. Its refusal is
    /// [`Setting::refuse`], which names the variable the deployment actually
    /// set, and it quotes the value only through [`Setting::shown`], which never
    /// repeats what a file held.
    ///
    /// A duration's key — one ending in `_SECS` or `_MS` — is refused here and
    /// at every reader built on this one: it is read through
    /// [`DurationBounds`](crate::DurationBounds).
    pub fn setting(&self, key: &str) -> Result<Option<Setting>, ConfigError> {
        self.refuse_duration(key)?;
        self.duration_setting(key)
    }

    /// [`setting`](Self::setting) without the duration check — the reader
    /// [`DurationBounds`](crate::DurationBounds) reads through, and nothing else.
    pub(crate) fn duration_setting(&self, key: &str) -> Result<Option<Setting>, ConfigError> {
        match self.spelled(key)? {
            None => Ok(None),
            Some(Spelled::Inline(value)) => {
                Ok(Some(Setting::new(value, self.var_name(key), false)))
            }
            Some(Spelled::File(path)) => {
                let var = self.var_name(&file_key(key));
                let bytes = self.read_file(key, &path)?;
                #[expect(
                    clippy::map_err_ignore,
                    reason = "FromUtf8Error carries the file's bytes, and the file may hold a secret"
                )]
                let text = String::from_utf8(bytes).map_err(|_| {
                    ConfigError::parse(var.clone(), "names a file that is not UTF-8 text")
                })?;
                let value = text.trim_end_matches(['\n', '\r']);
                Ok((!value.is_empty()).then(|| Setting::new(value.to_owned(), var, true)))
            }
        }
    }

    /// The bytes of `key` as the deployment gave them, and the file they came
    /// from — for a consumer that parses bytes rather than text (a certificate, a
    /// key) or re-reads the file when it is renewed. Resolved exactly as
    /// [`get`](Self::get) resolves it, with the bytes kept as read; a file holding
    /// nothing but line breaks is unset.
    ///
    /// Either spelling present in the deployment tier — empty included — shadows
    /// both in `.env`; both set within the tier that answers is refused naming
    /// both. The path is trimmed of ASCII whitespace and must name a regular file
    /// of at most a mebibyte; anything else is boot-fatal, naming the variable
    /// and never its value.
    pub fn material(&self, key: &str) -> Result<Option<Setting<Material>>, ConfigError> {
        self.refuse_duration(key)?;
        Ok(match self.spelled(key)? {
            None => None,
            Some(Spelled::Inline(value)) => Some(Setting::new(
                Material {
                    bytes: value.into_bytes(),
                    path: None,
                },
                self.var_name(key),
                false,
            )),
            Some(Spelled::File(path)) => {
                let bytes = self.read_file(key, &path)?;
                let blank = bytes.iter().all(|byte| matches!(byte, b'\n' | b'\r'));
                (!blank).then(|| {
                    Setting::new(
                        Material {
                            bytes,
                            path: Some(path),
                        },
                        self.var_name(&file_key(key)),
                        true,
                    )
                })
            }
        })
    }

    /// Refuse `key` when it names a duration, which only
    /// [`DurationBounds`](crate::DurationBounds) reads.
    fn refuse_duration(&self, key: &str) -> Result<(), ConfigError> {
        let key = key.to_ascii_uppercase();
        if key.ends_with("_SECS") || key.ends_with("_MS") {
            return Err(ConfigError::UnboundedDuration {
                var: self.var_name(&key),
            });
        }
        Ok(())
    }

    /// Which spelling of `key` the tier that answers set, refusing both.
    fn spelled(&self, key: &str) -> Result<Option<Spelled>, ConfigError> {
        let file_key = file_key(key);
        let (var, file_var) = (self.var_name(key), self.var_name(&file_key));
        self.record(&var);
        self.record(&file_var);
        let deployment_only = self.precedence == Precedence::OverPinned
            || self.source.in_deployment(&var)
            || self.source.in_deployment(&file_var);
        let read = |name: &str| {
            let value = if deployment_only {
                self.source.get_from_deployment(name)
            } else {
                self.source.get(name)
            };
            // Empty is unset whatever the source: a custom source that forgets
            // would otherwise blank a default or show two spellings.
            value.filter(|value| !value.is_empty())
        };
        match (read(&var), read(&file_var)) {
            (None, None) => Ok(None),
            (Some(value), None) => Ok(Some(Spelled::Inline(value))),
            (None, Some(path)) => Ok(Some(Spelled::File(PathBuf::from(
                path.trim_matches(|c: char| c.is_ascii_whitespace()),
            )))),
            (Some(_), Some(_)) => Err(ConfigError::parse(
                var,
                format!(
                    "is set together with {file_var} — give the value once, inline or as a path"
                ),
            )),
        }
    }

    fn read_file(&self, key: &str, path: &std::path::Path) -> Result<Vec<u8>, ConfigError> {
        read_material(path).map_err(|source| ConfigError::File {
            var: self.var_name(&file_key(key)),
            source,
        })
    }

    /// Record `var` for the config whose `from_env` is in flight and for the
    /// unclaimed-variable report. `var_name` must not record: it only cites.
    fn record(&self, var: &str) {
        crate::unclaimed::witness(var);
        READING.with(|cell| {
            if let Some(read) = cell.borrow_mut().as_mut() {
                read.insert(var.to_owned());
            }
        });
    }

    /// `Err` (naming the variable) when set-but-unparseable — boot-fatal, no
    /// silent fallback. [`Setting::parse`] words the refusal, so a value read
    /// from a file is never passed through the parser's reason.
    pub fn parse<T>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        self.setting(key)?
            .map(|setting| setting.parse())
            .transpose()
    }

    /// A structured value — a list of records, a map — written as JSON and
    /// decoded as `T`. Unset is `None`; a value that does not decode is
    /// boot-fatal naming the variable, and the refusal never quotes the value
    /// ([`Setting::json`]).
    pub fn json<T>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T: serde::de::DeserializeOwned,
    {
        self.setting(key)?.map(|setting| setting.json()).transpose()
    }

    /// `1`/`true`/`yes`/`on` and their negatives, case-insensitive.
    ///
    /// The vocabulary is [`nest_rs_core::parse_bool`], shared with the kernel; an
    /// unrecognised value is a boot error naming the variable.
    pub fn flag(&self, key: &str, default: bool) -> Result<bool, ConfigError> {
        match self.setting(key)? {
            None => Ok(default),
            Some(setting) => nest_rs_core::parse_bool(&setting.value).ok_or_else(|| {
                if setting.from_file() {
                    setting.refuse("expected a boolean in the file it names")
                } else {
                    setting.refuse(format_args!(
                        "expected a boolean, got `{}`",
                        setting.shown()
                    ))
                }
            }),
        }
    }

    /// A whole count, where **`0` means unlimited**: unset keeps `base`, and
    /// set-but-unparseable is boot-fatal naming the variable.
    pub fn count(&self, key: &str, base: Option<usize>) -> Result<Option<usize>, ConfigError> {
        Ok(match self.parse::<usize>(key)? {
            None => base,
            Some(0) => None,
            Some(count) => Some(count),
        })
    }

    /// Comma-separated, trimmed, empties dropped; `default` is kept when unset.
    pub fn list(&self, key: &str, default: Vec<String>) -> Result<Vec<String>, ConfigError> {
        Ok(self
            .get(key)?
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or(default))
    }
}

/// The two spellings a key is given in.
enum Spelled {
    Inline(String),
    File(PathBuf),
}

fn file_key(key: &str) -> String {
    format!("{key}_FILE")
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A source with a deployment tier over a cascade, the shape `EnvSource`
    /// serves, where a deployment entry may be present and empty.
    struct Tiers {
        deployment: std::collections::HashMap<String, String>,
        cascade: std::collections::HashMap<String, String>,
    }

    impl Tiers {
        fn new(deployment: &[(&str, &str)], cascade: &[(&str, &str)]) -> Arc<Self> {
            let named = |pairs: &[(&str, &str)]| {
                pairs
                    .iter()
                    .map(|(key, value)| (var_name("tiers", key), (*value).to_owned()))
                    .collect()
            };
            Arc::new(Self {
                deployment: named(deployment),
                cascade: named(cascade),
            })
        }
    }

    impl ConfigSource for Tiers {
        fn get(&self, var: &str) -> Option<String> {
            match self.deployment.get(var) {
                Some(value) => Some(value.clone()).filter(|v| !v.is_empty()),
                None => self.cascade.get(var).cloned(),
            }
        }
        fn get_from_deployment(&self, var: &str) -> Option<String> {
            self.deployment.get(var).cloned().filter(|v| !v.is_empty())
        }
        fn in_deployment(&self, var: &str) -> bool {
            self.deployment.contains_key(var)
        }
    }

    /// A deployment that blanks a variable unsets it under both spellings: a
    /// committed `_FILE` does not come back through the other name.
    #[test]
    fn a_blank_deployment_value_shadows_the_cascade_file_spelling() {
        let env = ConfigService::with_source(
            "tiers",
            Tiers::new(&[("SECRET", "")], &[("SECRET_FILE", "/nonexistent/secret")]),
        );
        assert_eq!(env.get("SECRET").unwrap(), None);
    }

    /// The deployment chooses the spelling, pinned or not: a deployment `_FILE`
    /// beside a committed inline value is the deployment's answer, not a refusal.
    #[test]
    fn a_deployment_spelling_shadows_the_other_spelling_in_the_cascade() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("secret", "from-the-deployment\n")?;
            let deployed_file =
                || Tiers::new(&[("SECRET_FILE", "secret")], &[("SECRET", "committed")]);
            let unpinned = ConfigService::with_source("tiers", deployed_file());
            assert_eq!(
                unpinned.get("SECRET").unwrap().as_deref(),
                Some("from-the-deployment")
            );
            let pinned = ConfigService::with_source("tiers", deployed_file()).over_pinned();
            assert_eq!(
                pinned.get("SECRET").unwrap().as_deref(),
                Some("from-the-deployment")
            );
            Ok(())
        });
    }

    /// An empty file is unset, as an empty variable is — so a default survives it.
    #[test]
    fn an_empty_file_is_unset() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("empty", "\n")?;
            let env =
                ConfigService::with_vars("fixture", [("LIST_FILE", "empty"), ("ON_FILE", "empty")]);
            assert_eq!(env.get("LIST").unwrap(), None);
            assert_eq!(
                env.list("LIST", vec!["default".into()]).unwrap(),
                ["default"]
            );
            assert!(env.flag("ON", true).unwrap());
            assert!(env.material("LIST").unwrap().is_none());
            Ok(())
        });
    }

    /// A refusal names the spelling the deployment set, and never repeats a value
    /// read from a file — `_FILE` is the secrets channel.
    #[test]
    fn a_value_from_a_file_is_refused_under_its_file_variable_and_never_shown() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("secret", "hunter2-SECRET")?;
            let env = ConfigService::with_vars(
                "fixture",
                [("ON_FILE", "secret"), ("PORT_FILE", "secret")],
            );
            let flag = env.flag("ON", false).unwrap_err().to_string();
            assert!(flag.contains(&var_name("fixture", "ON_FILE")), "{flag}");
            assert!(!flag.contains("hunter2"), "{flag}");
            let port = env.parse::<u16>("PORT").unwrap_err().to_string();
            assert!(port.contains(&var_name("fixture", "PORT_FILE")), "{port}");
            Ok(())
        });
    }

    /// A parser whose error quotes its input, as `T::Err` is free to.
    #[derive(Debug)]
    struct Echoing;
    impl FromStr for Echoing {
        type Err = String;
        fn from_str(s: &str) -> Result<Self, String> {
            Err(format!("unknown value {s:?}"))
        }
    }

    /// `parse` keeps the parser's reason for an inline value and drops it for
    /// one read from a file, whose content it may quote.
    #[test]
    fn parse_never_passes_a_file_value_through_the_parsers_reason() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("secret", "hunter2-SECRET")?;
            let from_file = ConfigService::with_vars("fixture", [("MODE_FILE", "secret")])
                .parse::<Echoing>("MODE")
                .unwrap_err()
                .to_string();
            assert!(!from_file.contains("hunter2"), "{from_file}");
            assert!(
                from_file.contains(&var_name("fixture", "MODE_FILE")),
                "{from_file}"
            );
            assert!(from_file.contains("does not parse as"), "{from_file}");

            let inline = ConfigService::with_vars("fixture", [("MODE", "sideways")])
                .parse::<Echoing>("MODE")
                .unwrap_err()
                .to_string();
            assert!(inline.contains("unknown value \"sideways\""), "{inline}");
            Ok(())
        });
    }

    /// A record a structured value lists: a client and the secret it carries.
    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Client {
        client_id: String,
        client_secret: String,
        scopes: Vec<String>,
    }

    /// `scopes` given as a string where a list is expected, beside a secret.
    const MISTYPED: &str =
        r#"[{"client_id":"ci","client_secret":"hunter2-SECRET","scopes":"hunter2-SCOPE"}]"#;

    #[test]
    fn json_decodes_a_structured_value_and_is_none_when_unset() {
        let env = ConfigService::with_vars(
            "fixture",
            [(
                "CLIENTS",
                r#"[{"client_id":"ci","client_secret":"s","scopes":["user"]}]"#,
            )],
        );
        let clients: Vec<Client> = env.json("CLIENTS").unwrap().expect("set");
        assert_eq!(
            clients,
            [Client {
                client_id: "ci".into(),
                client_secret: "s".into(),
                scopes: vec!["user".into()],
            }]
        );
        assert!(env.json::<Vec<Client>>("UNSET").unwrap().is_none());
    }

    /// serde's sentence quotes the value it refused — here a scope list given as
    /// a string, and beside it a client's secret. The refusal names the variable,
    /// says where and what kind of value it found, and quotes no byte of it,
    /// inline or from a file alike.
    #[test]
    fn a_structured_value_that_does_not_decode_is_refused_without_its_value() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("clients.json", MISTYPED)?;
            for (env, var) in [
                (
                    ConfigService::with_vars("fixture", [("CLIENTS", MISTYPED)]),
                    var_name("fixture", "CLIENTS"),
                ),
                (
                    ConfigService::with_vars("fixture", [("CLIENTS_FILE", "clients.json")]),
                    var_name("fixture", "CLIENTS_FILE"),
                ),
            ] {
                let err = env.json::<Vec<Client>>("CLIENTS").unwrap_err();
                assert!(
                    matches!(&err, ConfigError::Decode { var: named, .. } if *named == var),
                    "{err:?}"
                );
                let rendered = nest_rs_core::error_message(&err);
                assert!(rendered.contains(&var), "{rendered}");
                assert!(rendered.contains("invalid type: a string"), "{rendered}");
                assert!(rendered.contains("line 1 column"), "{rendered}");
                assert!(!rendered.contains("hunter2"), "{rendered}");
                assert!(!format!("{err:?}").contains("hunter2"), "{err:?}");
            }
            Ok(())
        });
    }

    /// A consumer's refusal names the spelling that supplied the value, and
    /// quotes a file's content only as a placeholder.
    #[test]
    fn a_setting_refuses_under_its_spelling_and_shows_a_file_value_as_a_placeholder() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("secret", "hunter2-SECRET\n")?;
            let env = ConfigService::with_vars("fixture", [("KEY_FILE", "secret"), ("OTHER", "x")]);

            let file = env.setting("KEY").unwrap().expect("present");
            assert_eq!(file.value, "hunter2-SECRET");
            assert!(file.from_file());
            assert_eq!(file.var(), var_name("fixture", "KEY_FILE"));
            let refused = file
                .refuse(format_args!("`{}` is no good", file.shown()))
                .to_string();
            assert!(!refused.contains("hunter2"), "{refused}");
            assert!(
                refused.contains(&var_name("fixture", "KEY_FILE")),
                "{refused}"
            );
            assert!(!format!("{file:?}").contains("hunter2"));

            let inline = env.setting("OTHER").unwrap().expect("present");
            assert!(!inline.from_file());
            assert_eq!(inline.var(), var_name("fixture", "OTHER"));
            assert_eq!(inline.shown().to_string(), "x");

            let material = env.material("KEY").unwrap().expect("present");
            assert_eq!(material.var(), var_name("fixture", "KEY_FILE"));
            assert!(material.from_file());
            Ok(())
        });
    }

    #[test]
    fn spellings_name_both_spellings_of_a_key() {
        let env = ConfigService::with_vars("fixture", []);
        assert_eq!(
            env.spellings("KEY"),
            format!(
                "{} (or {})",
                var_name("fixture", "KEY"),
                var_name("fixture", "KEY_FILE")
            )
        );
    }

    #[test]
    fn get_reads_any_variable_from_the_file_its_file_variable_names() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("secret", "s3cr3t\n")?;
            let env = ConfigService::with_vars("fixture", [("SECRET_FILE", "secret")]);
            assert_eq!(
                env.get("SECRET").unwrap().as_deref(),
                Some("s3cr3t"),
                "the file's trailing line break is not part of the value",
            );
            Ok(())
        });
    }

    #[test]
    fn get_refuses_a_file_that_is_not_utf8_naming_its_variable() {
        figment::Jail::expect_with(|jail| {
            std::fs::write(jail.directory().join("secret"), [0xff, 0xfe]).expect("write fixture");
            let err = ConfigService::with_vars("fixture", [("SECRET_FILE", "secret")])
                .get("SECRET")
                .unwrap_err();
            assert!(
                err.to_string()
                    .contains(&var_name("fixture", "SECRET_FILE")),
                "names the variable: {err}"
            );
            Ok(())
        });
    }

    #[test]
    fn get_given_both_spellings_is_refused_naming_both() {
        let err =
            ConfigService::with_vars("fixture", [("SECRET", "inline"), ("SECRET_FILE", "secret")])
                .get("SECRET")
                .unwrap_err()
                .to_string();
        assert!(err.contains(&var_name("fixture", "SECRET")), "{err}");
        assert!(err.contains(&var_name("fixture", "SECRET_FILE")), "{err}");
        assert!(!err.contains("s3cr3t"), "never prints the value: {err}");
    }

    #[test]
    fn material_reads_the_material_inline_or_from_the_file_its_file_variable_names() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("ca.pem", "-----FROM-FILE-----")?;

            let inline = ConfigService::with_vars("fixture", [("CA", "-----INLINE-----")])
                .material("CA")
                .expect("inline material reads")
                .expect("and is present");
            assert_eq!(inline.value.bytes, b"-----INLINE-----");
            assert_eq!(
                inline.value.path, None,
                "inline material has no file to watch"
            );

            let file = ConfigService::with_vars("fixture", [("CA_FILE", "ca.pem")])
                .material("CA")
                .expect("a readable file reads")
                .expect("and is present");
            assert_eq!(file.value.bytes, b"-----FROM-FILE-----");
            assert_eq!(file.value.path, Some(PathBuf::from("ca.pem")));

            let block = ConfigService::with_vars("fixture", [("CA_FILE", " ca.pem\n")])
                .material("CA")
                .expect("a YAML block scalar's trailing newline is not part of the path")
                .expect("and is present");
            assert_eq!(block.value.path, Some(PathBuf::from("ca.pem")));

            assert!(
                ConfigService::with_vars("fixture", [])
                    .material("CA")
                    .expect("unset is not an error")
                    .is_none()
            );
            Ok(())
        });
    }

    #[test]
    fn material_given_both_inline_and_as_a_path_is_refused_naming_both() {
        let err = ConfigService::with_vars(
            "fixture",
            [("CA", "-----INLINE-----"), ("CA_FILE", "ca.pem")],
        )
        .material("CA")
        .expect_err("two spellings of one material cannot both be meant");
        let rendered = err.to_string();
        assert!(
            rendered.contains(&var_name("fixture", "CA")),
            "names the inline variable: {rendered}"
        );
        assert!(
            rendered.contains(&var_name("fixture", "CA_FILE")),
            "and the path variable: {rendered}"
        );
    }

    /// A `_FILE` value is never printed: an operator who pastes the material
    /// itself — with its PEM header, or as bare base64 no shape test can tell
    /// from a path — would otherwise find the key in the boot error.
    #[test]
    fn material_names_the_variable_of_a_file_it_cannot_read_and_never_its_value() {
        for value in [
            "/nonexistent/nest-rs-config/ca.pem",
            "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIs3cr3tb4se64\n-----END PRIVATE KEY-----\n",
            "MC4CAQAwBQYDK2VwBCIEIs3cr3tb4se64",
        ] {
            let err = ConfigService::with_vars("fixture", [("CA_FILE", value)])
                .material("CA")
                .expect_err("an unreadable file is boot-fatal");
            let mut shown = vec![err.to_string(), format!("{err:?}")];
            let mut source = std::error::Error::source(&err);
            assert!(source.is_some(), "the io error travels as the source");
            while let Some(cause) = source {
                shown.push(cause.to_string());
                shown.push(format!("{cause:?}"));
                source = cause.source();
            }
            assert!(
                shown[0].contains(&var_name("fixture", "CA_FILE")),
                "names the variable: {}",
                shown[0]
            );
            let secret = value.trim().lines().nth(1).unwrap_or(value.trim());
            for text in &shown {
                assert!(!text.contains(secret), "never prints the value: {text}");
            }
        }
    }

    /// A FIFO blocks whoever opens it until something writes, so a `_FILE`
    /// naming one would hang the boot forever.
    #[cfg(unix)]
    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test tears down its FIFO best-effort, and the watchdog's receiver may be gone"
    )]
    fn material_refuses_a_path_that_is_not_a_regular_file_without_blocking() {
        let fifo = std::env::temp_dir().join(format!("nest-rs-config-fifo-{}", std::process::id()));
        let _ = std::fs::remove_file(&fifo);
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "a FIFO to point at");
        let path = fifo.to_str().expect("a UTF-8 path").to_owned();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let read = ConfigService::with_vars("fixture", [("CA_FILE", path.as_str())])
                .material("CA")
                .map(|pem| pem.is_some());
            let _ = tx.send(read.map_err(|e| e.to_string()));
        });
        let outcome = rx.recv_timeout(Duration::from_secs(5));
        let _ = std::fs::remove_file(&fifo);
        let refused = outcome.expect("returns instead of blocking on the pipe");
        assert!(refused.is_err(), "a FIFO is not material: {refused:?}");
    }

    #[test]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the test tears down its temp dir best-effort"
    )]
    fn material_reads_at_most_a_mebibyte() {
        let dir = std::env::temp_dir().join(format!("nest-rs-config-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let limit = usize::try_from(crate::material::MAX_MATERIAL_BYTES).expect("fits");
        let at = dir.join("at.pem");
        let over = dir.join("over.pem");
        std::fs::write(&at, vec![b'a'; limit]).expect("write");
        std::fs::write(&over, vec![b'a'; limit + 1]).expect("write");
        let read = |path: &std::path::Path| {
            ConfigService::with_vars("fixture", [("CA_FILE", path.to_str().expect("UTF-8"))])
                .material("CA")
        };
        let at_limit = read(&at);
        let over_limit = read(&over);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            at_limit
                .expect("the limit itself reads")
                .expect("present")
                .value
                .bytes
                .len(),
            limit
        );
        let err = over_limit.expect_err("one byte over is refused");
        let source = std::error::Error::source(&err)
            .and_then(|cause| cause.downcast_ref::<std::io::Error>())
            .expect("an io error says why");
        assert_eq!(source.kind(), std::io::ErrorKind::FileTooLarge, "{err}");
    }

    /// A custom source answering `Some("")` means unset, as the trait says —
    /// for `get` and so for `pem`, which must not see two spellings.
    #[test]
    fn an_empty_value_from_any_source_is_unset() {
        struct Blank;
        impl ConfigSource for Blank {
            fn get(&self, var: &str) -> Option<String> {
                if var == var_name("fixture", "CA_FILE") {
                    Some("ca.pem".into())
                } else {
                    Some(String::new())
                }
            }
        }
        figment::Jail::expect_with(|jail| {
            jail.create_file("ca.pem", "-----FROM-FILE-----")?;
            let env = ConfigService::with_source("fixture", Arc::new(Blank));
            assert_eq!(env.get("OTHER").unwrap(), None, "empty is unset");
            let pem = env
                .material("CA")
                .expect("an empty inline value is not a second spelling")
                .expect("present");
            assert_eq!(pem.value.bytes, b"-----FROM-FILE-----");
            Ok(())
        });
    }

    #[test]
    fn material_debug_never_shows_the_material() {
        let pem =
            ConfigService::with_vars("fixture", [("KEY", "-----BEGIN PRIVATE KEY-----s3cr3t")])
                .material("KEY")
                .expect("reads")
                .expect("present");
        let shown = format!("{pem:?}");
        assert!(!shown.contains("s3cr3t"), "{shown}");
    }

    #[test]
    fn var_name_builds_the_namespaced_name() {
        let env = ConfigService::for_namespace("seaorm");
        assert_eq!(env.var_name("URL"), var_name("seaorm", "URL"));
        assert_eq!(
            env.var_name("max_connections"),
            var_name("seaorm", "MAX_CONNECTIONS")
        );
    }

    #[test]
    fn the_free_var_name_matches_the_readers() {
        assert_eq!(var_name("seaorm", "url"), var_name("seaorm", "URL"));
        assert_eq!(
            var_name("redis", "CONNECT_TIMEOUT_SECS"),
            ConfigService::for_namespace("redis").var_name("connect_timeout_secs"),
        );
    }

    #[test]
    fn parse_reports_the_variable_on_failure() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("testdb", "MAX"), "not-a-number");
            let env = ConfigService::for_namespace("testdb");
            let err = env.parse::<u32>("MAX").expect_err("non-numeric must fail");
            assert!(
                matches!(err, ConfigError::Parse { ref var, .. } if *var == var_name("testdb", "MAX"))
            );
            Ok(())
        });
    }

    #[test]
    fn parse_is_none_when_unset() {
        figment::Jail::expect_with(|_| {
            let env = ConfigService::for_namespace("testdb");
            assert!(
                env.parse::<u32>("UNSET_KEY")
                    .expect("unset is Ok(None)")
                    .is_none()
            );
            Ok(())
        });
    }

    #[test]
    fn flag_reads_common_spellings() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("testf", "ON"), "yes");
            jail.set_env(var_name("testf", "OFF"), "false");
            let env = ConfigService::for_namespace("testf");
            assert!(env.flag("ON", false).unwrap());
            assert!(!env.flag("OFF", true).unwrap());
            assert!(env.flag("MISSING", true).unwrap());
            Ok(())
        });
    }

    #[test]
    fn list_splits_on_commas() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("testl", "SCOPES"), "read:user, write , ,admin");
            let env = ConfigService::for_namespace("testl");
            assert_eq!(
                env.list("SCOPES", Vec::new()).unwrap(),
                vec!["read:user", "write", "admin"],
            );
            Ok(())
        });
    }

    #[test]
    fn list_keeps_the_default_when_unset() {
        let env = ConfigService::with_vars("testl", []);
        assert_eq!(
            env.list("SCOPES", vec!["pinned".to_owned()]).unwrap(),
            vec!["pinned".to_owned()],
            "an unset list keeps the base, the same way `flag` keeps its default",
        );
    }

    #[test]
    fn over_pinned_narrows_to_the_deployment_tier() {
        let env = ConfigService::with_vars("prec", [("PORT", "9000")]);
        assert_eq!(env.get("PORT").unwrap().as_deref(), Some("9000"));
        assert_eq!(
            env.over_pinned().get("PORT").unwrap().as_deref(),
            Some("9000"),
            "a custom source is deployment-supplied unless it says otherwise",
        );
    }

    #[test]
    fn env_source_over_pinned_ignores_the_dotenv_cascade_but_not_the_real_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                &format!("{}=from_dotenv", var_name("precpin", "FROM_FILE")),
            )?;
            jail.set_env(var_name("precpin", "FROM_REAL"), "from_real");
            let pinned = ConfigService::for_namespace("precpin").over_pinned();
            assert_eq!(
                pinned.get("FROM_REAL").unwrap().as_deref(),
                Some("from_real"),
                "a deployment variable outranks a value pinned in code",
            );
            assert_eq!(
                pinned.get("FROM_FILE").unwrap(),
                None,
                "a committed .env file does not silently undo a deliberate pin",
            );
            assert_eq!(
                ConfigService::for_namespace("precpin")
                    .get("FROM_FILE")
                    .unwrap()
                    .as_deref(),
                Some("from_dotenv"),
            );
            Ok(())
        });
    }

    #[test]
    fn with_source_reads_from_the_custom_source_only() {
        use std::collections::HashMap;
        struct Map(HashMap<String, &'static str>);
        impl ConfigSource for Map {
            fn get(&self, var: &str) -> Option<String> {
                self.0.get(var).map(|s| (*s).to_owned())
            }
        }
        let source = Arc::new(Map(HashMap::from([(
            var_name("custom", "URL"),
            "value-from-map",
        )])));
        let env = ConfigService::with_source("custom", source);
        assert_eq!(env.get("URL").unwrap().as_deref(), Some("value-from-map"));
        assert!(env.get("MISSING").unwrap().is_none());
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the test asserts the process environment was left alone"
    )]
    fn with_source_does_not_load_dotenv_into_process_env() {
        struct Empty;
        impl ConfigSource for Empty {
            fn get(&self, _var: &str) -> Option<String> {
                None
            }
        }
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                &format!(
                    "{}=loaded-from-dotenv",
                    var_name("leak_guard", "SHOULD_STAY_UNSET"),
                ),
            )?;
            let env = ConfigService::with_source("leakguard", Arc::new(Empty));
            assert!(env.get("ANYTHING").unwrap().is_none());
            assert!(
                std::env::var(var_name("leak_guard", "SHOULD_STAY_UNSET")).is_err(),
                "custom-source path must not merge .env into the process env",
            );
            Ok(())
        });
    }

    #[test]
    fn count_reads_zero_as_the_unlimited_sentinel() {
        let base_count = Some(5usize);

        let unset = ConfigService::with_vars("probe", []);
        assert_eq!(unset.count("N", base_count).expect("unset"), base_count);

        let zero = ConfigService::with_vars("probe", [("N", "0")]);
        assert_eq!(
            zero.count("N", base_count).expect("zero"),
            None,
            "`0` is the unlimited sentinel, never a ceiling of zero",
        );

        let set = ConfigService::with_vars("probe", [("N", "7")]);
        assert_eq!(set.count("N", base_count).expect("set"), Some(7));

        for bad in ["-1", " 7 ", "abc", "3.0"] {
            let service = ConfigService::with_vars("probe", [("N", bad)]);
            let err = service
                .count("N", base_count)
                .expect_err("a set-but-unparseable ceiling is boot-fatal");
            assert!(
                err.to_string().contains("N"),
                "and it names the variable: {err}",
            );
        }
    }

    /// A duration is read through `DurationBounds` and nowhere else: every
    /// public reader refuses a `_SECS` or `_MS` key — set or not, in either
    /// case — naming `DurationBounds`, while the bounds themselves read it.
    #[test]
    fn every_public_reader_refuses_a_duration_key_naming_duration_bounds() {
        let env = ConfigService::with_vars("fixture", [("WINDOW_SECS", "30")]);
        for key in ["WINDOW_SECS", "window_secs", "DEADLINE_MS", "UNSET_SECS"] {
            let refusals = [
                env.get(key).map(drop),
                env.setting(key).map(drop),
                env.material(key).map(drop),
                env.parse::<u64>(key).map(drop),
                env.json::<u64>(key).map(drop),
                env.flag(key, false).map(drop),
                env.count(key, None).map(drop),
                env.list(key, Vec::new()).map(drop),
            ];
            for refused in refusals {
                let err = refused.expect_err("a duration key is refused");
                assert!(
                    matches!(err, ConfigError::UnboundedDuration { ref var } if *var == var_name("fixture", key)),
                    "{err:?}"
                );
                assert!(err.to_string().contains("`DurationBounds`"), "{err}");
            }
        }

        const WINDOW: crate::DurationBounds = crate::DurationBounds::secs(
            "WINDOW_SECS",
            "FixtureConfig::window",
            crate::Floor::AboveZero("why"),
            crate::Bound {
                count: 60,
                why: "why",
            },
        );
        assert_eq!(
            WINDOW
                .read(&env, std::time::Duration::from_secs(5))
                .expect("the bounds read it")
                .value,
            std::time::Duration::from_secs(30),
        );
        assert_eq!(env.get("WINDOWS").expect("not a duration"), None);
    }
}
