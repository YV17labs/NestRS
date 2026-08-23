//! Framework env-var scheme `<PREFIX>_<DOMAIN>__<KEY>` and the typed
//! [`ConfigService`] reader handed to a config's `from_env`.
//!
//! `<PREFIX>` is `NESTRS` unless the deployment named its own through
//! [`EnvPrefix::VAR`](nest_rs_core::EnvPrefix::VAR); every name in this crate is built
//! from [`var_name`], so the two can never drift.
//!
//! Domain = owning crate's name with the `nest-rs-` prefix stripped, and a crate
//! maps **its own**. Borrowing a sibling's variable — reading it as an explicit
//! fallback inside your `from_env` — used to be sanctioned here, and the claim
//! registry below now refuses it **through this reader**: `get` records every
//! name it resolves against the type whose `from_env` is in flight, so a second
//! `ConfigService` opened on another namespace claims that namespace's variable
//! and the owner's own read raises
//! [`ConfigError::ContestedVariable`](crate::ConfigError).
//!
//! **What it does not reach is the free [`env_var`](crate::env_var), and the
//! sentence is written that way because the shorter one was false.** That
//! function reads the environment without a reader, so no window is armed and
//! nothing is claimed — which is precisely the spelling
//! `docs/configuration/env-cascade` teaches for a borrow. Saying "borrowing is
//! a boot failure" flatly told a reader the framework refuses something it
//! waves through, and *"a `warn` whose sentence is wrong is worse than none"*
//! (`framework.md`) is the same rule one level up. Whether the free function
//! should be covered too is an **owner question**: it is called from places
//! with no config in flight at all, so covering it means deciding what an
//! unowned read means, not adding a line.
//!
//! Nothing in either workspace borrows today by either spelling, and
//! `nest-rs-throttler` declines to read HTTP's trusted-proxy list with the
//! reason written at the site — so the practice was already abandoned before it
//! was refused.
//!
//! **Recorded rather than quietly dropped**, because it narrows a documented
//! capability. The justification that stood here — "since the `.env` cascade is
//! merged once before any `from_env` runs" — was separately stale: resolving a
//! config never mutates the process environment any more.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::EnvPrefix;

use crate::error::ConfigError;
use crate::source::{ConfigSource, EnvSource, MapSource};

thread_local! {
    /// Every variable name read while a `Config::resolve` is in flight.
    ///
    /// A thread-local because `from_env` is synchronous and single-threaded by
    /// construction — it is one call, on the resolving thread, with a
    /// `&ConfigService` it may hand to as many inherent sub-readers as it likes.
    /// That is exactly why the recording sits on the *reader* rather than on the
    /// config type: `HttpConfig` delegates ten of its keys to `TlsConfig`,
    /// `CorsConfig` and `SecurityHeadersConfig`, none of which is a `Config`, and
    /// nothing that inspects types can see those reads.
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
/// **The resolved name, never the key.** A key reaches [`ConfigService::get`]
/// from a string literal, a `const`, a sub-struct's own `from_env` or an
/// expression built at the call site, so the only place the full
/// `<PREFIX>_<DOMAIN>__<KEY>` is knowable is where it is actually asked for.
/// This is also what makes the check exact where the namespace grammar is not:
/// `("social__google", "CLIENT_ID")` and `("social", "GOOGLE__CLIENT_ID")` are
/// two different key pairs and one variable, and it is the variable a
/// deployment sets — and the tree ships both spellings, since `nest-rs-social`
/// uses the separator as a nesting device.
///
/// **What it covers, and what it does not.** The window is armed by
/// [`Config::read`](crate::Config::read), which every path into a `Config`'s
/// `from_env` takes — the two `resolve` paths and a discovery registry reading
/// its plugin's namespace, i.e. all three rows of `architecture.md`'s
/// Configuration table. Inside it, a key reaches [`ConfigService::get`] from a
/// literal, a `const`, an inherent sub-struct's own `from_env` or an expression
/// built at the call site, and all four are recorded identically because what
/// is recorded is the resolved name.
///
/// It does **not** cover a namespace read with no `Config` type behind it:
/// `nest-rs-opentelemetry` builds `OpenTelemetryConfig` by hand from a bare
/// `ConfigService::for_namespace("opentelemetry")`, because it runs *before the
/// container exists* — it is what builds the subscriber. Nothing owns those
/// seven variables, and nothing can until that struct is a `Config`; that is an
/// owner question, not a hole this seam can close, and it is stated here rather
/// than left for a reader to discover.
///
/// **A join over the source was tried and removed**, which is why the argument
/// is recorded rather than assumed. It could see neither a namespace claimed
/// through [`ConfigService::for_namespace`] without a `#[config]`, nor a key
/// read from a `const`, nor the ten `HttpConfig` keys that reach the reader
/// through inherent sub-structs — and it recorded two variables that do not
/// exist, because a `var_name` in an error message looks exactly like a read.
/// Every one of those is a *green*-staying gap, which is the direction a check
/// may not fail in. Three of the four are closed here; the fourth is the
/// paragraph above.
pub(crate) fn claiming<C: 'static, T>(load: impl FnOnce() -> T) -> (T, Result<(), ConfigError>) {
    /// Restores the outer window even if `from_env` unwinds.
    ///
    /// `from_env` is developer code and a panic there is catchable in practice —
    /// a tokio task boundary, `figment::Jail::expect_with`, a `#[should_panic]`.
    /// Restoring after the call left the thread-local armed on that path, so
    /// `READING` never returned to `None` on that thread and every later bare
    /// `ConfigService::get` accumulated into a set nothing would ever claim.
    ///
    /// **Deliberately unpinned**, and stated rather than tested: every
    /// `claiming` replaces the cell on entry, so the orphan set changes no
    /// claim and no refusal — the only consequence is unbounded growth on a
    /// thread that panicked mid-read. Nothing public can observe that, and a
    /// test that would stay green through the guard's removal is worse than
    /// none (`testing.md` clause 3). The guard is hygiene, and it is here
    /// because the alternative is a leak nobody can see.
    struct Window(Option<BTreeSet<String>>);
    impl Drop for Window {
        fn drop(&mut self) {
            READING.with(|cell| *cell.borrow_mut() = self.0.take());
        }
    }

    // **The identity is the `TypeId`; the name is only for the sentence.** It
    // was `std::any::type_name`, whose own documentation says the returned
    // string "must not be considered to uniquely identify a type" and "is not a
    // stable identifier" — so two types whose names collide (one crate linked
    // at two semver-majors, a module duplicated by `#[path]`) read as one owner
    // and the contest this exists to catch was waved through. `access.rs`'s
    // `ProviderDescriptor` is the shape already in the tree: a `TypeId` decides,
    // a `&'static str` appears in the message.
    let owner = Owner {
        id: std::any::TypeId::of::<C>(),
        name: std::any::type_name::<C>(),
    };
    let outer = Window(READING.with(|cell| cell.replace(Some(BTreeSet::new()))));
    let value = load();
    let read = READING.with(|cell| cell.take()).unwrap_or_default();
    drop(outer);

    // `CLAIMED` is touched by this loop alone and the loop cannot panic, so the
    // poisoned arm is unreachable today. It is written rather than
    // `expect`-ed because the fail direction is the one that matters if that
    // ever stops being true: a missed diagnostic is a diagnostic, while
    // refusing to boot over a poisoned bookkeeping mutex would make this check
    // an outage of its own.
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
/// The primitive [`ConfigService::var_name`] delegates to, exposed for the
/// places that must cite a variable with no reader in hand — a `Validate` impl,
/// a `thiserror` message, a boot check on a pinned struct. Hardcoding the name
/// there would print a variable that does not exist under a custom prefix,
/// which is the one thing an operator reads such a message for.
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

/// Which tiers of the environment outrank the value a field falls back to.
///
/// A `Config` is always resolved as *environment over a base*. What the base
/// **is** decides how much of the environment may overrule it, which is the
/// whole of the framework's precedence rule:
/// `real env > pinned in code > .env cascade > in-code defaults`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Precedence {
    /// The base is the config's own defaults, so every tier the source serves
    /// outranks it — the real process env first, then the `.env` cascade.
    #[default]
    OverDefaults,
    /// The base is a value pinned at the call site (`Module::for_root(cfg)`), so
    /// only [`ConfigSource::get_from_deployment`] outranks it. A `.env` file
    /// committed beside the code does not silently undo a deliberate pin; a
    /// deployment variable always does.
    OverPinned,
}

/// Typed reader bound to one namespace; resolves `<PREFIX>_<NAMESPACE>__<KEY>`.
pub struct ConfigService {
    namespace: String,
    source: Arc<dyn ConfigSource>,
    precedence: Precedence,
}

impl ConfigService {
    /// A reader scoped to `namespace`, backed by the process/`.env` environment.
    pub fn for_namespace(namespace: &str) -> Self {
        Self::with_source(namespace, Arc::new(EnvSource))
    }

    /// Build a reader backed by a custom [`ConfigSource`]. The `.env` cascade
    /// is **not** merged — the source is the sole authority for resolution,
    /// and the process env stays untouched (no global side effect from
    /// constructing this reader).
    pub fn with_source(namespace: &str, source: Arc<dyn ConfigSource>) -> Self {
        Self {
            // Stored verbatim: `var_name` uppercases both segments, so casing
            // here would only be a second pass over the same bytes.
            namespace: namespace.to_owned(),
            source,
            precedence: Precedence::OverDefaults,
        }
    }

    /// Narrow this reader to the tiers that outrank a **code-pinned** value.
    /// Called by [`Config::resolve`](crate::Config::resolve) when the call site
    /// passed a config to `Module::for_root`, so a field the deployment sets
    /// still wins while the `.env` cascade defers to the pin.
    pub fn over_pinned(mut self) -> Self {
        self.precedence = Precedence::OverPinned;
        self
    }

    /// Convenience over [`with_source`](Self::with_source) + [`MapSource`]: a
    /// reader backed by an in-memory map, keyed by **`<KEY>` alone** — the same
    /// string [`get`](Self::get) is asked for. Resolves hermetically (no process
    /// env, no `.env`), so config tests and fixtures need no
    /// `unsafe { std::env::set_var }`. An empty `vars` yields all in-code
    /// defaults.
    ///
    /// **It took fully-qualified names, and that was the defect.** The caller
    /// re-performed a join this reader already owns
    /// (`get` → [`var_name`]), so a fixture and the reader could disagree — and
    /// the disagreeing spelling was the shorter one, which 53 call sites in 16
    /// crates picked. Under `NESTRS_ENV_PREFIX=ACME` the reader looked for
    /// `ACME_APP__PORT` while the fixture wrote `NESTRS_APP__PORT`, so 70 tests
    /// across the workspace failed — and the ones that did not fail passed by
    /// asserting nothing. Keying on `<KEY>` makes the wrong thing unspellable.
    ///
    /// [`MapSource`] keeps its full-name contract, deliberately: it stands in
    /// for the environment, where names *are* fully qualified. This is the
    /// convenience built on top, and its job is that a fixture cannot mean
    /// something the reader does not.
    ///
    /// ```
    /// # use nest_rs_config::ConfigService;
    /// let cfg = ConfigService::with_vars("app", [("PORT", "8080")]);
    /// assert_eq!(cfg.get("PORT").as_deref(), Some("8080"));
    /// assert_eq!(cfg.get("MISSING"), None);
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

    /// The raw string value for `key` in this namespace, or `None` if unset in
    /// every tier this reader's precedence lets through.
    pub fn get(&self, key: &str) -> Option<String> {
        let var = self.var_name(key);
        // The one funnel: `parse`, `flag`, `seconds`, `count` and `list` all
        // reach the environment through here, so recording once records every
        // read. `var_name` deliberately does not record — it *cites* a variable
        // in a message (sometimes a glob, `TLS_*`), which is not a claim on one.
        READING.with(|cell| {
            if let Some(read) = cell.borrow_mut().as_mut() {
                read.insert(var.clone());
            }
        });
        match self.precedence {
            Precedence::OverDefaults => self.source.get(&var),
            Precedence::OverPinned => self.source.get_from_deployment(&var),
        }
    }

    /// `Err` (naming the variable) when set-but-unparseable — boot-fatal, no
    /// silent fallback.
    pub fn parse<T>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        match self.get(key) {
            None => Ok(None),
            Some(raw) => raw
                .parse::<T>()
                .map(Some)
                .map_err(|e| ConfigError::parse(self.var_name(key), e.to_string())),
        }
    }

    /// `1`/`true`/`yes`/`on` and their negatives, case-insensitive.
    ///
    /// The vocabulary is [`nest_rs_core::parse_bool`], not a copy of it: this
    /// crate reads every `<PREFIX>_<NS>__<KEY>` boolean a deployment writes, and
    /// the kernel reads `<PREFIX>_LOG_SOURCE_LOCATION` before a container
    /// exists, so the two must answer one grammar. What is this crate's own is
    /// the *unrecognised* case — a `#[config]` reports the value back as a boot
    /// error naming the variable, where a subscriber has no error path and takes
    /// its default.
    pub fn flag(&self, key: &str, default: bool) -> Result<bool, ConfigError> {
        match self.get(key) {
            None => Ok(default),
            Some(raw) => nest_rs_core::parse_bool(&raw).ok_or_else(|| {
                ConfigError::parse(
                    self.var_name(key),
                    format!("expected a boolean, got `{raw}`"),
                )
            }),
        }
    }

    /// Whole seconds, where **`0` means off** — the spelling every long-lived
    /// connection in the framework bounds itself with.
    ///
    /// Unset keeps `base`, `0` is the off/unlimited sentinel, and a
    /// set-but-unparseable value is boot-fatal naming the variable. It lives
    /// here rather than beside any one of them because the sentinel is a
    /// security control — the ceiling on how long a connection replays the
    /// privileges it authenticated with once — and four crates each reading `0`
    /// their own way is four chances for one of them to read it as *zero
    /// seconds* and turn a ceiling into a kill switch.
    pub fn seconds(
        &self,
        key: &str,
        base: Option<Duration>,
    ) -> Result<Option<Duration>, ConfigError> {
        Ok(match self.parse::<u64>(key)? {
            None => base,
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
        })
    }

    /// A whole count, where **`0` means unlimited** — [`seconds`](Self::seconds)'
    /// spelling for a ceiling that bounds a *quantity* rather than a duration.
    ///
    /// Same three cases and the same reason they live here: unset keeps `base`,
    /// `0` is the unlimited sentinel, and set-but-unparseable is boot-fatal
    /// naming the variable. A ceiling on how many units of work one request may
    /// ask for is a security control, and `0` read as *zero allowed* would turn
    /// it into a kill switch — which is exactly the misreading one shared
    /// spelling exists to prevent.
    pub fn count(&self, key: &str, base: Option<usize>) -> Result<Option<usize>, ConfigError> {
        Ok(match self.parse::<usize>(key)? {
            None => base,
            Some(0) => None,
            Some(count) => Some(count),
        })
    }

    /// Comma-separated, trimmed, empties dropped. `default` is the value the
    /// field keeps when the variable is unset — the same shape as
    /// [`flag`](Self::flag), so a `from_env` body passes `base.<field>` and the
    /// overlay reads the same way for every field type.
    pub fn list(&self, key: &str, default: Vec<String>) -> Vec<String> {
        self.get(key)
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or(default)
    }
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    #[test]
    fn var_name_builds_the_namespaced_name() {
        let env = ConfigService::for_namespace("seaorm");
        assert_eq!(env.var_name("URL"), var_name("seaorm", "URL"));
        assert_eq!(
            env.var_name("max_connections"),
            var_name("seaorm", "MAX_CONNECTIONS")
        );
    }

    // The readerless primitive must agree with the method, since the two are
    // what an operator compares: an error message built one way and a `.env`
    // line built the other.
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
                env.list("SCOPES", Vec::new()),
                vec!["read:user", "write", "admin"],
            );
            Ok(())
        });
    }

    #[test]
    fn list_keeps_the_default_when_unset() {
        let env = ConfigService::with_vars("testl", []);
        assert_eq!(
            env.list("SCOPES", vec!["pinned".to_owned()]),
            vec!["pinned".to_owned()],
            "an unset list keeps the base, the same way `flag` keeps its default",
        );
    }

    // The precedence split D-2 rests on: a pinned value loses to a deployment
    // variable and wins over the `.env` cascade. `MapSource` stands in for a
    // custom source, whose default is "deployment-supplied" — the fail-safe
    // direction, so a Vault value is never shadowed by a pinned struct.
    #[test]
    fn over_pinned_narrows_to_the_deployment_tier() {
        let env = ConfigService::with_vars("prec", [("PORT", "9000")]);
        assert_eq!(env.get("PORT").as_deref(), Some("9000"));
        assert_eq!(
            env.over_pinned().get("PORT").as_deref(),
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
                pinned.get("FROM_REAL").as_deref(),
                Some("from_real"),
                "a deployment variable outranks a value pinned in code",
            );
            assert_eq!(
                pinned.get("FROM_FILE"),
                None,
                "a committed .env file does not silently undo a deliberate pin",
            );
            // Unpinned, the cascade is back in play.
            assert_eq!(
                ConfigService::for_namespace("precpin")
                    .get("FROM_FILE")
                    .as_deref(),
                Some("from_dotenv"),
            );
            Ok(())
        });
    }

    // A `with_source` reader bypasses the env entirely — pin that the source
    // is the sole authority so a third-party Vault/ConfigMap impl is not
    // shadowed by stale process env.
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
        assert_eq!(env.get("URL").as_deref(), Some("value-from-map"));
        assert!(env.get("MISSING").is_none());
    }

    // The dotenv cascade used to fire from `for_namespace`, which meant any
    // `ConfigService` — including one built on a custom source — would
    // permanently merge `.env` into the process env. Pin that a non-env
    // source never triggers the merge: `.env` exists in the jail with a
    // marker, and after a `with_source` read, that marker must still be
    // unset in `std::env`.
    #[test]
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
            // Build + use the custom-source reader. If dotenv leaked here it
            // would set the marker in the jailed process env.
            let env = ConfigService::with_source("leakguard", Arc::new(Empty));
            assert!(env.get("ANYTHING").is_none());
            assert!(
                std::env::var(var_name("leak_guard", "SHOULD_STAY_UNSET")).is_err(),
                "custom-source path must not merge .env into the process env",
            );
            Ok(())
        });
    }

    /// `count` and `seconds` are one spelling of one sentinel; the tests are
    /// paired for the same reason the doc comments are — a divergence would mean
    /// `0` reading as *off* on one and *zero allowed* on the other, which is the
    /// misreading the shared helper exists to prevent.
    #[test]
    fn count_and_seconds_read_zero_as_the_same_sentinel() {
        let base_count = Some(5usize);
        let base_secs = Some(Duration::from_secs(5));

        let unset = ConfigService::with_vars("probe", []);
        assert_eq!(unset.count("N", base_count).expect("unset"), base_count);
        assert_eq!(unset.seconds("N", base_secs).expect("unset"), base_secs);

        let zero = ConfigService::with_vars("probe", [("N", "0")]);
        assert_eq!(
            zero.count("N", base_count).expect("zero"),
            None,
            "`0` is the unlimited sentinel, never a ceiling of zero",
        );
        assert_eq!(zero.seconds("N", base_secs).expect("zero"), None);

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
}
