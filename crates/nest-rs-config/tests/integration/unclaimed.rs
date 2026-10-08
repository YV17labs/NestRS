//! The unclaimed-variable report, through the path a boot takes: a `#[config]`
//! resolved from the environment, the events read back off the capture.
//!
//! Each test owns its process (nextest) for the once-per-process ledger, and
//! reads the events for its own variables only: the first read also checks the
//! ambient environment.

use nest_rs_config::unclaimed::{MISSPELLED_CONFIG_NAMESPACE, UNREAD_CONFIG_VARIABLE};
use nest_rs_config::{Config, ConfigService, Environment, config, var_name};
use nest_rs_core::EnvPrefix;
use nest_rs_testing::{CapturedEvent, LogCapture};

#[config(namespace = "unclaimed_keys")]
#[derive(Clone, Default)]
struct KeysConfig {
    port: u16,
    label: String,
}

impl Config for KeysConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            port: env.parse("PORT")?.unwrap_or(base.port),
            label: env.get("LABEL")?.unwrap_or(base.label),
        })
    }
}

/// A family member: its namespace carries the family as a level.
#[config(namespace = "unclaimed__member")]
#[derive(Clone, Default)]
struct MemberConfig {
    url: String,
}

impl Config for MemberConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            url: env.get("URL")?.unwrap_or(base.url),
        })
    }
}

/// Makes `<PREFIX>_LOG_FORMAT` equal a name read here once separators are set
/// aside, so the framework-wide names stay silent only by being known.
#[config(namespace = "log")]
#[derive(Clone, Default)]
struct LogShapedConfig {
    level: String,
    format: String,
    source_location: String,
}

impl Config for LogShapedConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            level: env.get("LEVEL")?.unwrap_or(base.level),
            format: env.get("FORMAT")?.unwrap_or(base.format),
            source_location: env.get("SOURCE_LOCATION")?.unwrap_or(base.source_location),
        })
    }
}

#[config(namespace = "env")]
#[derive(Clone, Default)]
struct EnvShapedConfig {
    prefix: String,
}

impl Config for EnvShapedConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            prefix: env.get("PREFIX")?.unwrap_or(base.prefix),
        })
    }
}

/// The events `message` filed for `variable`.
fn reports(logs: &LogCapture, message: &str, variable: &str) -> Vec<CapturedEvent> {
    logs.find(nest_rs_config::TARGET, message)
        .into_iter()
        .filter(|event| event.field("variable").as_deref() == Some(variable))
        .collect()
}

/// The one event `message` filed for `variable`.
fn report(logs: &LogCapture, message: &str, variable: &str) -> CapturedEvent {
    match reports(logs, message, variable).as_slice() {
        [event] => event.clone(),
        other => panic!(
            "expected exactly one `{message}` for {variable}, got {}: {:#?}",
            other.len(),
            logs.events(),
        ),
    }
}

/// Neither event, for `variable`.
fn assert_silent(logs: &LogCapture, variable: &str) {
    for message in [UNREAD_CONFIG_VARIABLE, MISSPELLED_CONFIG_NAMESPACE] {
        assert!(
            reports(logs, message, variable).is_empty(),
            "{variable} must not be reported: {:#?}",
            logs.events(),
        );
    }
}

/// The typo the report exists for: the value lands nowhere, and the event names
/// the variable the deployment meant.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_misspelled_key_is_reported_with_the_key_that_was_read() {
    figment::Jail::expect_with(|jail| {
        let typo = var_name("unclaimed_keys", "PROT");
        jail.set_env(&typo, "8080");
        let logs = LogCapture::install();

        let config = KeysConfig::load().expect("the config loads");

        assert_eq!(config.port, 0, "the misspelled value reached nothing");
        let event = report(&logs, UNREAD_CONFIG_VARIABLE, &typo);
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("namespace").as_deref(), Some("unclaimed_keys"));
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed_keys", "PORT")),
        );
        Ok(())
    });
}

/// A key nothing read and nothing near: still reported, since the namespace is
/// this binary's own — only the suggestion is withheld.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_key_with_no_near_neighbour_is_reported_without_a_suggestion() {
    figment::Jail::expect_with(|jail| {
        let stray = var_name("unclaimed_keys", "BANNER_COLOUR");
        jail.set_env(&stray, "teal");
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");

        let event = report(&logs, UNREAD_CONFIG_VARIABLE, &stray);
        assert_eq!(event.field("suggestion"), None, "{event:?}");
        Ok(())
    });
}

/// The `_FILE` spelling of a key is the same variable, so it is known — and a
/// typo in it is answered with the spelling that exists.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_file_spelling_is_known_and_its_typo_is_answered() {
    figment::Jail::expect_with(|jail| {
        jail.create_file("label", "from-a-file\n")?;
        let file = var_name("unclaimed_keys", "LABEL_FILE");
        let typo = var_name("unclaimed_keys", "PORT_FLIE");
        jail.set_env(&file, "label");
        jail.set_env(&typo, "port");
        let logs = LogCapture::install();

        let config = KeysConfig::load().expect("the config loads");

        assert_eq!(config.label, "from-a-file");
        assert_silent(&logs, &file);
        let event = report(&logs, UNREAD_CONFIG_VARIABLE, &typo);
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed_keys", "PORT_FILE")),
        );
        Ok(())
    });
}

/// A family-level rename: the variable keeps the former spelling, and the event
/// names the spelling that is read.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_namespace_spelled_with_other_separators_is_reported_under_the_linked_spelling() {
    figment::Jail::expect_with(|jail| {
        let former = var_name("unclaimed_member", "URL");
        jail.set_env(&former, "https://former.example");
        let logs = LogCapture::install();

        let config = MemberConfig::load().expect("the config loads");

        assert_eq!(config.url, "", "the former spelling reached nothing");
        let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, &former);
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("namespace").as_deref(),
            Some("unclaimed__member")
        );
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed__member", "URL")),
        );
        Ok(())
    });
}

/// The namespace half waits for the namespace's own read, since the keys it
/// reads are what tell a misspelling from another binary's variable — and runs
/// at that read, ended by an error or not, so a renamed *required* variable is
/// named ahead of the boot error its absence causes.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_namespace_is_checked_once_its_config_is_read() {
    figment::Jail::expect_with(|jail| {
        let former = var_name("unclaimed_member", "URL");
        jail.set_env(&former, "https://former.example");
        let logs = LogCapture::install();

        KeysConfig::load().expect("another config loads first");
        assert_silent(&logs, &former);

        MemberConfig::load().expect("the member loads");
        let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, &former);
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed__member", "URL")),
        );
        Ok(())
    });
}

/// Stand-ins for `openapi`, `authn` and `ws` (the real ones may be linked too,
/// and a namespace belongs to one type).
#[config(namespace = "unclaimed_openapi")]
#[derive(Clone, Default)]
struct OpenapiShapedConfig {
    title: String,
}

impl Config for OpenapiShapedConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            title: env.get("TITLE")?.unwrap_or(base.title),
        })
    }
}

#[config(namespace = "authq")]
#[derive(Clone, Default)]
struct AuthnShapedConfig {
    issuer: String,
}

impl Config for AuthnShapedConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            issuer: env.get("ISSUER")?.unwrap_or(base.issuer),
        })
    }
}

#[config(namespace = "wx")]
#[derive(Clone, Default)]
struct WsShapedConfig {
    url: String,
}

impl Config for WsShapedConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            url: env.get("URL")?.unwrap_or(base.url),
        })
    }
}

/// Another binary's namespace one edit from a linked one (`OPENAI`, `AUTH`,
/// `ES`) is silent: a short namespace has no typo reach, and a long one is
/// reported only under a key it reads.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn another_binarys_namespaces_beside_a_short_or_unread_key_are_left_alone() {
    figment::Jail::expect_with(|jail| {
        const SECRET: &str = "sk-live-SECRETVALUE";
        let others = [
            var_name("unclaimed_openai", "API_KEY"),
            var_name("auth", "ISSUER"),
            var_name("authz", "POLICY"),
            var_name("ex", "URL"),
        ];
        let typo = var_name("unclaimed_opneapi", "TITLE");
        for name in others.iter().chain([&typo]) {
            jail.set_env(name, SECRET);
        }
        let logs = LogCapture::install();

        OpenapiShapedConfig::load().expect("the config loads");
        AuthnShapedConfig::load().expect("the config loads");
        WsShapedConfig::load().expect("the config loads");

        for other in &others {
            assert_silent(&logs, other);
        }
        let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, &typo);
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed_openapi", "TITLE"))
        );
        assert!(
            logs.events()
                .iter()
                .all(|event| !format!("{event:?}").contains(SECRET)),
            "no value reaches any event",
        );
        Ok(())
    });
}

/// A shared `.env` serves several binaries, so what this one does not link is
/// another's: a namespace it has never heard of, and a member of one of its
/// namespaces that it does not link, are both left alone.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn another_binarys_variables_are_left_alone() {
    figment::Jail::expect_with(|jail| {
        let elsewhere = var_name("unclaimed_elsewhere", "URL");
        let member = var_name("unclaimed_keys__worker", "CONCURRENCY");
        let control = var_name("unclaimed_keys", "PROT");
        jail.set_env(&elsewhere, "https://elsewhere.example");
        jail.set_env(&member, "4");
        jail.set_env(&control, "8080");
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");
        MemberConfig::load().expect("the config loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &control);
        assert_silent(&logs, &elsewhere);
        assert_silent(&logs, &member);
        Ok(())
    });
}

/// A linked config nobody reads has no known keys, and its variables reach
/// nothing in this binary — so they are not this binary's to report, whether
/// they misspell a key or run the level separator into it.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_linked_config_that_is_never_read_files_no_key_report() {
    figment::Jail::expect_with(|jail| {
        let unread = var_name("unclaimed__member", "URLL");
        let run_together = var_name("unclaimed__member", "URL").replace("__", "_");
        let control = var_name("unclaimed_keys", "PROT");
        jail.set_env(&unread, "https://typo.example");
        jail.set_env(&run_together, "https://run-together.example");
        jail.set_env(&control, "8080");
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &control);
        assert_silent(&logs, &unread);
        assert_silent(&logs, &run_together);
        Ok(())
    });
}

/// The framework-wide names stay silent; the controls prove both comparisons
/// ran over this very environment.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn framework_wide_variables_are_never_reported() {
    figment::Jail::expect_with(|jail| {
        // First, so the prefix resolves to the default and the bootstrap's own
        // name sits under it — the one arrangement in which it could be seen.
        jail.set_env(EnvPrefix::VAR, "");
        let names = [
            EnvPrefix::VAR.to_owned(),
            EnvPrefix::var(nest_rs_core::logging::var::FILTER),
            EnvPrefix::var(nest_rs_core::logging::var::FORMAT),
            EnvPrefix::var(nest_rs_core::logging::var::SOURCE_LOCATION),
            Environment::var_name(),
        ];
        for name in &names {
            jail.set_env(name, "");
        }
        let unread = var_name("log", "LEVL");
        let run_together = var_name("log", "LEVEL").replace("__", "_");
        jail.set_env(&unread, "debug");
        jail.set_env(&run_together, "debug");
        let logs = LogCapture::install();

        LogShapedConfig::load().expect("the config loads");
        EnvShapedConfig::load().expect("the config loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &unread);
        report(&logs, MISSPELLED_CONFIG_NAMESPACE, &run_together);
        for name in &names {
            assert_silent(&logs, name);
        }
        Ok(())
    });
}

/// The level separator written as a word one — `SEAORM_URL` for `SEAORM__URL`,
/// the spelling every `DATABASE_URL` teaches — names no namespace, so it is
/// compared with the names read here whole: equal to one once separators are
/// set aside, it is answered with it; equal to none, it is left alone.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_run_together_name_is_answered_with_the_name_that_is_read() {
    figment::Jail::expect_with(|jail| {
        let port = var_name("unclaimed_keys", "PORT");
        let url = var_name("unclaimed__member", "URL");
        let port_typo = port.replace("__", "_");
        let url_typo = url.replace("__", "_");
        let nobody = var_name("unclaimed_elsewhere", "URL").replace("__", "_");
        jail.set_env(&port_typo, "8080");
        jail.set_env(&url_typo, "https://run-together.example");
        jail.set_env(&nobody, "https://nobody.example");
        let logs = LogCapture::install();

        let keys = KeysConfig::load().expect("the config loads");
        let member = MemberConfig::load().expect("the config loads");

        assert_eq!((keys.port, member.url.as_str()), (0, ""), "neither reached");
        for (typo, namespace, read) in [
            (&port_typo, "unclaimed_keys", port),
            (&url_typo, "unclaimed__member", url),
        ] {
            let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, typo);
            assert_eq!(event.field("namespace").as_deref(), Some(namespace));
            assert_eq!(event.field("suggestion"), Some(read));
        }
        assert_silent(&logs, &nobody);
        Ok(())
    });
}

/// A config loaded ahead of any subscriber — in `main`, before the one an `App`
/// installs — leaves what it found to the first read something hears, instead
/// of marking it reported to nobody.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_read_nobody_hears_leaves_its_report_to_the_first_one_heard() {
    figment::Jail::expect_with(|jail| {
        let typo = var_name("unclaimed_keys", "PROT");
        let former = var_name("unclaimed_member", "URL");
        jail.set_env(&typo, "8080");
        jail.set_env(&former, "https://former.example");

        KeysConfig::load().expect("the config loads with nothing listening");
        let logs = LogCapture::install();
        MemberConfig::load().expect("the next config loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &typo);
        report(&logs, MISSPELLED_CONFIG_NAMESPACE, &former);
        Ok(())
    });
}

/// A name is a name: the value it carries appears in no message and no field,
/// whichever shape it was reported under.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn the_value_is_never_reported() {
    figment::Jail::expect_with(|jail| {
        const SECRET: &str = "hunter2-unclaimed-secret";
        let typo = var_name("unclaimed_keys", "LABLE");
        let former = var_name("unclaimed_member", "URL");
        let run_together = var_name("unclaimed_keys", "LABEL").replace("__", "_");
        jail.set_env(&typo, SECRET);
        jail.set_env(&former, SECRET);
        jail.set_env(&run_together, SECRET);
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");
        MemberConfig::load().expect("the member loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &typo);
        report(&logs, MISSPELLED_CONFIG_NAMESPACE, &former);
        report(&logs, MISSPELLED_CONFIG_NAMESPACE, &run_together);
        for event in logs.events() {
            assert!(!event.message.contains(SECRET), "{event:?}");
            assert!(
                event.fields.values().all(|value| !value.contains(SECRET)),
                "{event:?}",
            );
        }
        Ok(())
    });
}

/// Once per variable, however many reads see it: a second load of the same
/// config, and a read of another, add no second line.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_variable_is_reported_once() {
    figment::Jail::expect_with(|jail| {
        let typo = var_name("unclaimed_keys", "PROT");
        let former = var_name("unclaimed_member", "URL");
        jail.set_env(&typo, "8080");
        jail.set_env(&former, "https://former.example");
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");
        KeysConfig::load().expect("the config loads again");
        MemberConfig::load().expect("another config loads");

        assert_eq!(reports(&logs, UNREAD_CONFIG_VARIABLE, &typo).len(), 1);
        assert_eq!(
            reports(&logs, MISSPELLED_CONFIG_NAMESPACE, &former).len(),
            1
        );
        Ok(())
    });
}

/// A reader on a custom source says nothing about what the deployment
/// exported, so it reports nothing — even for a variable the environment does
/// carry, and that the environment-backed read after it does report.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_reader_on_a_custom_source_reports_nothing() {
    figment::Jail::expect_with(|jail| {
        let typo = var_name("unclaimed_keys", "PROT");
        jail.set_env(&typo, "8080");
        let logs = LogCapture::install();

        nest_rs_config::read(
            &ConfigService::with_vars("unclaimed_keys", [("PORT", "9090")]),
            KeysConfig::default(),
        )
        .expect("the config reads from its fixture");
        assert_silent(&logs, &typo);

        KeysConfig::load().expect("the config loads from the environment");
        report(&logs, UNREAD_CONFIG_VARIABLE, &typo);
        Ok(())
    });
}

/// Under a deployment's own prefix, only its own names are examined: the same
/// typo spelled under the default prefix belongs to nobody here.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn under_a_custom_prefix_only_its_own_names_are_examined() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(EnvPrefix::VAR, "ACME");
        let typo = var_name("unclaimed_keys", "PROT");
        let default_spelling = format!(
            "{}{}",
            EnvPrefix::DEFAULT,
            typo.strip_prefix(EnvPrefix::current())
                .expect("the name is built from the declared prefix"),
        );
        jail.set_env(&typo, "8080");
        jail.set_env(&default_spelling, "8080");
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");

        let event = report(&logs, UNREAD_CONFIG_VARIABLE, &typo);
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed_keys", "PORT"))
        );
        assert_silent(&logs, &default_spelling);
        Ok(())
    });
}

/// The loader is exact, so a namespace or a prefix spelled in another case
/// configures nothing, and is reported with the name the loader reads.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_namespace_or_a_prefix_in_another_case_is_reported_with_the_name_read() {
    figment::Jail::expect_with(|jail| {
        let port = var_name("unclaimed_keys", "PORT");
        let url = var_name("unclaimed__member", "URL");
        let mixed = port.replace("UNCLAIMED_KEYS", "Unclaimed_Keys");
        let family = url.replace("UNCLAIMED__MEMBER", "Unclaimed__Member");
        let lowered = port.to_ascii_lowercase();
        for name in [&mixed, &family, &lowered] {
            jail.set_env(name, "8080");
        }
        let logs = LogCapture::install();

        let config = KeysConfig::load().expect("the config loads");
        MemberConfig::load().expect("the member loads");

        assert_eq!(
            config.port, 0,
            "no spelling in another case reached the field"
        );
        for (name, suggestion) in [(&mixed, &port), (&family, &url), (&lowered, &port)] {
            let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, name);
            assert_eq!(
                event.field("suggestion").as_deref(),
                Some(suggestion.as_str())
            );
        }
        Ok(())
    });
}

/// A letter typo in a namespace — in a family member's segment too — is one
/// edit from a namespace this binary links, and is reported like a key's typo
/// is; a sibling member differing by a whole word is another binary's and
/// stays silent.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_misspelled_namespace_is_reported_with_the_namespace_linked() {
    figment::Jail::expect_with(|jail| {
        let swapped = var_name("unclaimde_keys", "PORT");
        let member = var_name("unclaimed__membr", "URL");
        let sibling = var_name("unclaimed__other", "URL");
        for name in [&swapped, &member, &sibling] {
            jail.set_env(name, "x");
        }
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");
        MemberConfig::load().expect("the member loads");

        let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, &swapped);
        assert_eq!(event.field("namespace").as_deref(), Some("unclaimed_keys"));
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed_keys", "PORT"))
        );
        let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, &member);
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed__member", "URL"))
        );
        assert_silent(&logs, &sibling);
        Ok(())
    });
}
