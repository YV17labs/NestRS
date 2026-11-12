//! Every key 7.0 renamed, walked off the upgrading guide's table and read by
//! the config that now reads it: said retired, naming what reads it now, and
//! never read — a value under a retired key lands nowhere.
#![expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]

use nest_rs_config::{Config, var_name};
use nest_rs_core::__private::RETIRED_VARIABLE;
use nest_rs_graphql::GraphqlConfig;
use nest_rs_http::HttpConfig;
use nest_rs_oauth_client::OAuthClientConfig;
use nest_rs_oauth_resource::OAuthResourceConfig;
use nest_rs_oauth_server::OAuthServerConfig;
use nest_rs_openapi::OpenApiConfig;
use nest_rs_queue::QueueConfig;
use nest_rs_storage::StorageConfig;
use nest_rs_testing::LogCapture;

/// A namespace and a key.
type Key = (&'static str, &'static str);

/// The namespace and key each row retired, and what reads it now.
const RENAMED: &[(Key, Key)] = &[
    (
        ("oauth_client", "CLIENT_ID"),
        ("oauth__client", "CLIENT_ID"),
    ),
    (
        ("oauth_client", "CLIENT_SECRET"),
        ("oauth__client", "CLIENT_SECRET"),
    ),
    (("oauth_client", "AUTH_URL"), ("oauth__client", "AUTH_URL")),
    (
        ("oauth_client", "TOKEN_URL"),
        ("oauth__client", "TOKEN_URL"),
    ),
    (
        ("oauth_client", "REDIRECT_URL"),
        ("oauth__client", "REDIRECT_URL"),
    ),
    (
        ("oauth_client", "USERINFO_URL"),
        ("oauth__client", "USERINFO_URL"),
    ),
    (("oauth_client", "SCOPES"), ("oauth__client", "SCOPES")),
    (
        ("oauth_resource", "RESOURCE"),
        ("oauth__resource", "RESOURCE"),
    ),
    (
        ("oauth_resource", "AUTHORIZATION_SERVERS"),
        ("oauth__resource", "AUTHORIZATION_SERVERS"),
    ),
    (
        ("oauth_resource", "SCOPES_SUPPORTED"),
        ("oauth__resource", "SCOPES_SUPPORTED"),
    ),
    (
        ("oauth_resource", "BEARER_METHODS_SUPPORTED"),
        ("oauth__resource", "BEARER_METHODS_SUPPORTED"),
    ),
    (
        ("oauth_resource", "RESOURCE_NAME"),
        ("oauth__resource", "RESOURCE_NAME"),
    ),
    (
        ("oauth_resource", "RESOURCE_DOCUMENTATION"),
        ("oauth__resource", "RESOURCE_DOCUMENTATION"),
    ),
    (
        ("oauth_resource", "RESOURCE_POLICY_URI"),
        ("oauth__resource", "RESOURCE_POLICY_URI"),
    ),
    (
        ("oauth_resource", "RESOURCE_TOS_URI"),
        ("oauth__resource", "RESOURCE_TOS_URI"),
    ),
    (("oauth", "CLIENTS"), ("oauth__server", "CLIENTS")),
    (("storage", "BUCKET"), ("storage", "URL")),
    (("http", "CORS_MAX_AGE"), ("http", "CORS_MAX_AGE_SECS")),
    (("http", "CORS_EXPOSED"), ("http", "CORS_EXPOSED_HEADERS")),
    (
        ("http", "SECURITY_HEADERS"),
        ("http", "SECURITY_HEADERS_ENABLED"),
    ),
    (
        ("redis__worker", "SHUTDOWN_TIMEOUT_SECS"),
        ("queue", "SHUTDOWN_TIMEOUT_SECS"),
    ),
    (("graphql", "EMIT_SDL"), ("graphql", "EMIT")),
    (("graphql", "SCHEMA_PATH"), ("graphql", "EMIT_PATH")),
    (("openapi", "EMIT_DOCUMENT"), ("openapi", "EMIT")),
    (("openapi", "DOCUMENT_PATH"), ("openapi", "EMIT_PATH")),
];

/// A read of every config a row names; the storage read refuses its unset URL
/// after the report has run, so its answer says nothing here.
fn read_every_config() {
    let _ = OAuthClientConfig::resolve(None);
    let _ = OAuthResourceConfig::resolve(None);
    let _ = OAuthServerConfig::<()>::resolve(None);
    let _ = StorageConfig::resolve(None);
    let _ = HttpConfig::resolve(None);
    let _ = QueueConfig::resolve(None);
    let _ = GraphqlConfig::resolve(None);
    let _ = OpenApiConfig::resolve(None);
}

#[test]
fn every_key_7_0_renamed_is_said_retired_naming_what_reads_it_now() {
    figment::Jail::expect_with(|jail| {
        for ((namespace, key), _) in RENAMED {
            jail.set_env(var_name(namespace, key), "set-under-its-6x-key");
        }
        let logs = LogCapture::install();
        read_every_config();
        let lines = logs.find(nest_rs_config::TARGET, RETIRED_VARIABLE);
        for ((namespace, key), (read_namespace, read_key)) in RENAMED {
            let variable = var_name(namespace, key);
            let line = lines
                .iter()
                .find(|line| line.field("variable").as_deref() == Some(variable.as_str()))
                .unwrap_or_else(|| panic!("{variable} is said retired: {:#?}", logs.events()));
            assert_eq!(line.level, "warn", "{variable}");
            assert_eq!(
                line.field("read_as"),
                Some(var_name(read_namespace, read_key)),
                "{variable}"
            );
            assert_eq!(
                line.field("namespace").as_deref(),
                Some(*read_namespace),
                "{variable}"
            );
        }
        for (message, what) in [
            (
                nest_rs_config::unclaimed::UNREAD_CONFIG_VARIABLE,
                "read by no config",
            ),
            (
                nest_rs_config::unclaimed::MISSPELLED_CONFIG_NAMESPACE,
                "a misspelled namespace",
            ),
        ] {
            let guessed: Vec<_> = logs
                .find(nest_rs_config::TARGET, message)
                .into_iter()
                .filter(|line| {
                    RENAMED.iter().any(|((namespace, key), _)| {
                        line.field("variable").as_deref() == Some(&*var_name(namespace, key))
                    })
                })
                .collect();
            assert!(
                guessed.is_empty(),
                "a retired key is no {what}: {guessed:#?}"
            );
        }
        Ok(())
    });
}

/// A value under a retired key lands nowhere: a detector, never an alias.
#[test]
fn a_retired_key_is_never_read() {
    figment::Jail::expect_with(|jail| {
        jail.set_env(
            var_name("graphql", "SCHEMA_PATH"),
            "under-the-old-key.graphql",
        );
        jail.set_env(
            var_name("openapi", "DOCUMENT_PATH"),
            "under-the-old-key.json",
        );
        jail.set_env(var_name("redis__worker", "SHUTDOWN_TIMEOUT_SECS"), "7");
        assert_eq!(
            GraphqlConfig::resolve(None).expect("read").emit_path,
            GraphqlConfig::default().emit_path,
        );
        assert_eq!(
            OpenApiConfig::resolve(None).expect("read").emit_path,
            OpenApiConfig::default().emit_path,
        );
        assert_eq!(
            QueueConfig::resolve(None).expect("read").shutdown_timeout,
            QueueConfig::default().shutdown_timeout,
        );
        Ok(())
    });
}

/// A key removed with no successor is said retired, with no `read_as` to name.
#[test]
fn a_key_removed_with_nothing_in_its_place_is_said_retired_alone() {
    let variable = var_name("http", "FAIL_SECURE_STRICT");
    figment::Jail::expect_with(|jail| {
        jail.set_env(&variable, "false");
        let logs = LogCapture::install();
        HttpConfig::resolve(None).expect("an unread variable is no refusal");
        let line = logs.expect_one(nest_rs_config::TARGET, RETIRED_VARIABLE);
        assert_eq!(line.field("variable").as_deref(), Some(variable.as_str()));
        assert_eq!(line.field("read_as"), None);
        Ok(())
    });
}

/// Where losing a key would leave a policy weaker than the one set, the boot
/// refuses it in the words every retired variable is said with, its value
/// never among them: GraphQL's switch, whose successor is its opposite, and
/// each security header.
#[test]
fn a_key_whose_loss_weakens_a_policy_fails_the_boot_in_the_same_words() {
    const SECRET: &str = "default-src 'self' hunter2_secret";
    let cases = [
        ("graphql", "DISABLE_INTROSPECTION", "INTROSPECTION"),
        ("http", "HSTS", "SECURITY_HEADERS_HSTS"),
        (
            "http",
            "CONTENT_SECURITY_POLICY",
            "SECURITY_HEADERS_CONTENT_SECURITY_POLICY",
        ),
    ];
    for (namespace, key, read_as) in cases {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name(namespace, key), SECRET);
            let refused = match namespace {
                "graphql" => GraphqlConfig::resolve(None).map(drop),
                _ => HttpConfig::resolve(None).map(drop),
            }
            .expect_err("a retired key that weakens a policy is refused");
            assert!(
                matches!(&refused, nest_rs_config::ConfigError::Retired { .. }),
                "{refused:?}"
            );
            let refused = refused.to_string();
            assert!(
                refused.starts_with(&format!(
                    "{} is read by nothing since 7.0 — set {}:",
                    var_name(namespace, key),
                    var_name(namespace, read_as),
                )),
                "{refused}"
            );
            assert!(!refused.contains(SECRET), "{refused}");
            Ok(())
        });
    }
}

/// The refusal names the spelling that set it, as every refusal of a value does.
#[test]
fn a_refused_key_given_as_a_file_is_refused_under_its_file_spelling() {
    figment::Jail::expect_with(|jail| {
        jail.create_file("disable", "true")?;
        let file = var_name("graphql", "DISABLE_INTROSPECTION_FILE");
        jail.set_env(&file, "disable");
        let refused = GraphqlConfig::resolve(None)
            .map(drop)
            .expect_err("either spelling is the key")
            .to_string();
        assert!(refused.starts_with(&file), "{refused}");
        Ok(())
    });
}

/// One `.env` in a process of its own, since the cascade is parsed once.
fn cascade_setting(jail: &mut figment::Jail, variable: &str) -> figment::error::Result<()> {
    jail.create_file(".env", &format!("{variable}=true\n"))?;
    Ok(())
}

/// A pin cuts its namespace off the cascade, so a refused key a `.env` sets
/// under one weakens nothing: said retired, in the words of every other retired
/// key, and never told to export what would fail the boot.
#[test]
fn a_refused_key_the_cascade_sets_under_a_pin_is_said_retired() {
    let variable = var_name("graphql", "DISABLE_INTROSPECTION");
    figment::Jail::expect_with(|jail| {
        cascade_setting(jail, &variable)?;
        let logs = LogCapture::install();
        GraphqlConfig::resolve(Some(GraphqlConfig::default()))
            .expect("the pin holds the policy, and the file reaches nothing");
        let line = logs.expect_one(nest_rs_config::TARGET, RETIRED_VARIABLE);
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("variable").as_deref(), Some(variable.as_str()));
        assert_eq!(
            line.field("read_as"),
            Some(var_name("graphql", "INTROSPECTION"))
        );
        let shadowed = logs.find(
            nest_rs_config::TARGET,
            nest_rs_config::unclaimed::SHADOWED_CONFIG_VARIABLE,
        );
        assert!(
            shadowed.is_empty(),
            "its remedy fails the boot: {shadowed:#?}"
        );
        Ok(())
    });
}

/// The same for a security header's flat key: every refused key takes one path.
#[test]
fn a_refused_header_the_cascade_sets_under_a_pin_is_said_retired() {
    let variable = var_name("http", "HSTS");
    figment::Jail::expect_with(|jail| {
        cascade_setting(jail, &variable)?;
        let logs = LogCapture::install();
        HttpConfig::resolve(Some(HttpConfig::default()))
            .expect("the pin holds the policy, and the file reaches nothing");
        let line = logs.expect_one(nest_rs_config::TARGET, RETIRED_VARIABLE);
        assert_eq!(line.field("variable").as_deref(), Some(variable.as_str()));
        assert_eq!(
            line.field("read_as"),
            Some(var_name("http", "SECURITY_HEADERS_HSTS"))
        );
        Ok(())
    });
}

/// The deployment outranks a pin, so what it sets under one is refused as it
/// is with none.
#[test]
fn a_refused_key_the_deployment_sets_under_a_pin_fails_the_boot() {
    let variable = var_name("graphql", "DISABLE_INTROSPECTION");
    figment::Jail::expect_with(|jail| {
        jail.set_env(&variable, "true");
        let refused = GraphqlConfig::resolve(Some(GraphqlConfig::default()))
            .map(drop)
            .expect_err("the deployment outranks the pin");
        assert!(
            matches!(&refused, nest_rs_config::ConfigError::Retired { var, .. } if *var == variable),
            "{refused:?}"
        );
        Ok(())
    });
}

/// With no pin the cascade answers, so what it sets is refused, and the refusal
/// is the one sentence: the report that runs before it ends the boot adds none.
#[test]
fn a_refused_key_the_cascade_sets_with_no_pin_fails_the_boot_and_is_no_line() {
    let variable = var_name("graphql", "DISABLE_INTROSPECTION");
    figment::Jail::expect_with(|jail| {
        cascade_setting(jail, &variable)?;
        let logs = LogCapture::install();
        let refused = GraphqlConfig::resolve(None)
            .map(drop)
            .expect_err("the cascade answers");
        assert!(
            matches!(&refused, nest_rs_config::ConfigError::Retired { var, .. } if *var == variable),
            "{refused:?}"
        );
        let said = logs.find(nest_rs_config::TARGET, RETIRED_VARIABLE);
        assert!(said.is_empty(), "{said:#?}");
        Ok(())
    });
}

/// A deployment blanking a variable unsets it, so nothing is refused, and a name
/// no config reads is said like any other retired key.
#[test]
fn a_refused_key_a_deployment_blanks_is_said_retired() {
    let variable = var_name("graphql", "DISABLE_INTROSPECTION");
    figment::Jail::expect_with(|jail| {
        jail.set_env(&variable, "");
        let logs = LogCapture::install();
        GraphqlConfig::resolve(None).expect("a blank is unset");
        let line = logs.expect_one(nest_rs_config::TARGET, RETIRED_VARIABLE);
        assert_eq!(line.field("variable").as_deref(), Some(variable.as_str()));
        Ok(())
    });
}

/// A refused key is read by nothing, so nothing suggests it: the near miss of one
/// is told no key that would fail the boot.
#[test]
fn no_suggestion_names_a_refused_key() {
    let refused = var_name("graphql", "DISABLE_INTROSPECTION");
    let typo = var_name("graphql", "DISABLE_INTROSPECTIO");
    figment::Jail::expect_with(|jail| {
        jail.set_env(&typo, "true");
        let logs = LogCapture::install();
        GraphqlConfig::resolve(None).expect("an unread variable is no refusal");
        let lines = logs.find(
            nest_rs_config::TARGET,
            nest_rs_config::unclaimed::UNREAD_CONFIG_VARIABLE,
        );
        assert!(
            lines
                .iter()
                .any(|line| line.field("variable").as_deref() == Some(typo.as_str())),
            "the near miss is said: {lines:#?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| line.field("suggestion").as_deref() != Some(refused.as_str())),
            "{lines:#?}"
        );
        Ok(())
    });
}
