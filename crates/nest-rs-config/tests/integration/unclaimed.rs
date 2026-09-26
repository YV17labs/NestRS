//! The unclaimed-variable report, through the path a boot takes: a `#[config]`
//! resolved from the environment, the events read back off the capture.
//!
//! Every variable here is built — `var_name`, `EnvPrefix::var`,
//! `Environment::var_name` — so each test asserts the name a deployment under
//! the active prefix would actually export. Each test owns its process
//! (nextest), which is what makes the once-per-process ledger its own; and each
//! reads the events *for its own variables*, because the first read also checks
//! every namespace this suite links against whatever the ambient environment
//! carries.

use nest_rs_config::unclaimed::{MISSPELLED_CONFIG_NAMESPACE, UNREAD_CONFIG_VARIABLE};
use nest_rs_config::{Config, ConfigService, Environment, config, var_name};
use nest_rs_core::EnvPrefix;
use nest_rs_testing::{CapturedEvent, LogCapture};

/// A config shaped like the framework's own: two keys, read unconditionally.
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

/// A family member: its namespace carries the family as a level, the shape
/// 7.0 gave `oauth__client` and `oauth__resource`.
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

/// Namespaces the framework-wide names begin with, read by a config, so the
/// silence asserted for them is a property of the grammar and not of an
/// unread namespace.
#[config(namespace = "log")]
#[derive(Clone, Default)]
struct LogShapedConfig {
    level: String,
}

impl Config for LogShapedConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            level: env.get("LEVEL")?.unwrap_or(base.level),
        })
    }
}

#[config(namespace = "env")]
#[derive(Clone, Default)]
struct EnvShapedConfig {
    name: String,
}

impl Config for EnvShapedConfig {
    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        Ok(Self {
            name: env.get("NAME")?.unwrap_or(base.name),
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
#[allow(clippy::result_large_err)] // figment::Jail's fixed closure signature
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
#[allow(clippy::result_large_err)]
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
#[allow(clippy::result_large_err)]
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

/// The family-level rename: the variable keeps the 6.x spelling, the config
/// reads the default, and the event names the spelling that is read.
#[test]
#[allow(clippy::result_large_err)]
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

/// The namespace half reads the link-time registry, not the reads: a linked
/// config that no module has read yet is checked at the first read of any, so a
/// renamed *required* variable is named before the boot error it causes.
#[test]
#[allow(clippy::result_large_err)]
fn a_linked_namespace_is_checked_before_its_config_is_read() {
    figment::Jail::expect_with(|jail| {
        let former = var_name("unclaimed_member", "URL");
        jail.set_env(&former, "https://former.example");
        let logs = LogCapture::install();

        KeysConfig::load().expect("another config loads first");

        let event = report(&logs, MISSPELLED_CONFIG_NAMESPACE, &former);
        assert_eq!(
            event.field("suggestion"),
            Some(var_name("unclaimed__member", "URL")),
        );
        Ok(())
    });
}

/// A shared `.env` serves several binaries, so what this one does not link is
/// another's: a namespace it has never heard of, and a member of one of its
/// namespaces that it does not link, are both left alone.
#[test]
#[allow(clippy::result_large_err)]
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
/// nothing in this binary — so they are not this binary's to report.
#[test]
#[allow(clippy::result_large_err)]
fn a_linked_config_that_is_never_read_files_no_key_report() {
    figment::Jail::expect_with(|jail| {
        let unread = var_name("unclaimed__member", "URLL");
        let control = var_name("unclaimed_keys", "PROT");
        jail.set_env(&unread, "https://typo.example");
        jail.set_env(&control, "8080");
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &control);
        assert_silent(&logs, &unread);
        Ok(())
    });
}

/// The framework-wide names carry no `__` after the prefix, so no namespace can
/// hold them — not even one whose config reads a namespace they begin with.
/// Built from the constants that name them.
#[test]
#[allow(clippy::result_large_err)]
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
        let control = var_name("log", "LEVL");
        jail.set_env(&control, "debug");
        let logs = LogCapture::install();

        LogShapedConfig::load().expect("the config loads");
        EnvShapedConfig::load().expect("the config loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &control);
        for name in &names {
            assert_silent(&logs, name);
        }
        Ok(())
    });
}

/// A name is a name: the value it carries appears in no message and no field,
/// whichever shape it was reported under.
#[test]
#[allow(clippy::result_large_err)]
fn the_value_is_never_reported() {
    figment::Jail::expect_with(|jail| {
        const SECRET: &str = "hunter2-unclaimed-secret";
        let typo = var_name("unclaimed_keys", "LABLE");
        let former = var_name("unclaimed_member", "URL");
        jail.set_env(&typo, SECRET);
        jail.set_env(&former, SECRET);
        let logs = LogCapture::install();

        KeysConfig::load().expect("the config loads");

        report(&logs, UNREAD_CONFIG_VARIABLE, &typo);
        report(&logs, MISSPELLED_CONFIG_NAMESPACE, &former);
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
#[allow(clippy::result_large_err)]
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
#[allow(clippy::result_large_err)]
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
#[allow(clippy::result_large_err)]
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
