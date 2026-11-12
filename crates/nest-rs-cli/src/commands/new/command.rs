//! The `nestrs new` command: infer the layout from the tree and scaffold it
//! through the [`workspace`] strategy — a fresh monorepo, or an app added to
//! one that already exists.

//! **One starter, no template flag.** Every layout writes the shared
//! [`hello`](crate::templates::hello) module — a service with a greeting and a
//! `#[public] GET /`, so a fresh project proves it started.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::workspace;
use crate::context::{DEFAULT_ENV_PREFIX, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::Names;
use crate::scaffold::{Renderer, Scaffold};
use crate::templates::shared;

#[derive(Debug, Clone)]
pub(crate) struct NewOptions {
    pub name: String,
    pub output: PathBuf,
    /// `None` ⇒ the framework default (`NESTRS`).
    pub env_prefix: Option<String>,
    pub dry_run: bool,
}

pub(crate) fn run(opts: NewOptions) -> CliResult<()> {
    crate::naming::validate_feature_name(&opts.name).map_err(CliError::InvalidFeatureName)?;
    let names = Names::parse(&opts.name);

    if let Some(prefix) = &opts.env_prefix {
        crate::context::validate_env_prefix(prefix)
            .map_err(|e| CliError::Anyhow(anyhow::anyhow!(e)))?;
    }
    let env_prefix = opts.env_prefix.as_deref().unwrap_or(DEFAULT_ENV_PREFIX);

    if let Some(ws) = NestrsWorkspace::discover(&opts.output)? {
        // The prefix belongs to the deployment: an app added to an existing project
        // inherits it, and ignoring the flag silently would mislead the caller.
        if opts.env_prefix.is_some() {
            return Err(CliError::Anyhow(anyhow::anyhow!(
                "`--env-prefix` applies to project creation only — an app added to an \
                 existing workspace inherits the project's prefix. Set `{}` in the \
                 environment that runs it (the `Justfile`, your container, your shell).",
                crate::context::ENV_PREFIX_VAR,
            )));
        }
        return workspace::scaffold_app(&ws, &names, opts.dry_run);
    }

    workspace::scaffold_root(&opts.output, &names, env_prefix, opts.dry_run)
}

/// Seed every prefix placeholder — the value templates interpolate into
/// variable names, and the two lines that *set* it for the processes this
/// project starts.
/// Both setters are empty on the default. The `.env` cascade does **not** carry
/// it: the prefix selects the cascade, and the framework aborts on one inside it.
pub(crate) fn with_env_prefix(r: Renderer, env_prefix: &str) -> Renderer {
    prefix_vars(env_prefix)
        .into_iter()
        .fold(r, |r, (key, value)| r.with(key, value))
}

/// Every renderer key whose value depends on the project's env prefix, in one
/// list so the default seed (`Renderer::new`) and the `--env-prefix` override
/// cannot disagree. The note is rendered here because substitution is one pass:
/// a raw `{{env_prefix}}` inside a seeded value would survive it.
pub(crate) fn prefix_vars(env_prefix: &str) -> Vec<(&'static str, String)> {
    // `{{env_prefix_var}}` does not match `{{env_prefix}}`, so order is irrelevant.
    let fill = |template: &str| {
        template
            .replace("{{env_prefix_var}}", crate::context::ENV_PREFIX_VAR)
            .replace("{{env_prefix}}", env_prefix)
    };
    let default = env_prefix == DEFAULT_ENV_PREFIX;
    vec![
        ("env_prefix", env_prefix.to_owned()),
        ("dev_recipe_note", fill(shared::DEV_RECIPE_NOTE)),
        (
            "env_prefix_export",
            if default {
                String::new()
            } else {
                fill(shared::ENV_PREFIX_JUSTFILE)
            },
        ),
    ]
}

/// Queue the committed `.env` cascade (`.env`, `.env.development`, `.env.test`,
/// `.env.example`).
///
/// Every key in those files is written through `{{env_prefix}}`, so a project
/// created with `--env-prefix` gets a cascade its app actually reads.
pub(crate) fn queue_env_files(s: &mut Scaffold, base: &Path, r: &Renderer) {
    s.create_if_missing(base.join(".env"), r.render(shared::ENV));
    for (file, profile, own) in [
        (
            ".env.development",
            shared::ENV_DEVELOPMENT,
            shared::DEVELOPMENT_SERVICES,
        ),
        (".env.test", shared::ENV_TEST, shared::TEST_SERVICES),
    ] {
        let body = format!("{profile}\n{}{own}", shared::DEV_SERVICES);
        s.create_if_missing(base.join(file), r.render(&body));
    }
    s.create_if_missing(base.join(".env.example"), r.render(shared::ENV_EXAMPLE));
}

/// Queue the two files that carry the project's conventions: `AGENTS.md`, the
/// format every coding agent reads, and a `CLAUDE.md` that imports it (Claude
/// Code reads only the latter).
///
/// Assembled as `INTRO + LAYOUT + BODY`; the body embeds the architecture rules
/// verbatim.
pub(crate) fn queue_agent_files(s: &mut Scaffold, base: &Path, r: &Renderer) {
    let body = format!(
        "{}{}{}",
        shared::AGENTS_INTRO,
        shared::AGENTS_LAYOUT,
        shared::AGENTS_BODY
    );
    s.create(base.join("AGENTS.md"), r.render(&body));
    s.create(base.join("CLAUDE.md"), r.render(shared::CLAUDE_POINTER));
}

pub(crate) fn run_cargo_check(project_dir: &Path) -> CliResult<()> {
    let status = Command::new("cargo")
        .arg("check")
        .current_dir(project_dir)
        .status()
        .map_err(CliError::spawn("cargo"))?;
    if !status.success() {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "cargo check failed in {}",
            project_dir.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use nest_rs_config::{ConfigService, Environment, load_cascade};

    use super::{queue_env_files, with_env_prefix};
    use crate::naming::Names;
    use crate::scaffold::{Renderer, Scaffold};
    use crate::templates::{hello, workspace};

    /// The env profiles `nestrs new shop` writes, written into `dir` under the
    /// env prefix `prefix`, and the renderer that wrote them.
    fn write_profiles(dir: &Path, prefix: &str) -> Renderer {
        let rendering = with_env_prefix(Renderer::new(&Names::parse("shop")), prefix);
        let mut scaffold = Scaffold::new();
        queue_env_files(&mut scaffold, dir, &rendering);
        scaffold.apply(false).expect("the profiles are written");
        rendering
    }

    /// The logical database a Redis URL selects: its path, `0` when it has none.
    fn database(url: &str) -> u8 {
        let (_, authority_and_path) = url.split_once("://").expect("a Redis URL");
        authority_and_path
            .split_once('/')
            .map_or(0, |(_, index)| index.parse().expect("a database index"))
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail's closure signature is fixed"
    )]
    fn the_generated_test_profile_enqueues_on_a_redis_database_development_never_drains() {
        let prefix = nest_rs_core::EnvPrefix::current();
        figment::Jail::expect_with(|jail| {
            write_profiles(jail.directory(), prefix);
            let mut redis = |profile| {
                jail.clear_env();
                load_cascade(jail.directory(), profile);
                ConfigService::for_namespace("redis")
                    .get("URL")
                    .expect("the Redis URL reads")
                    .expect("the profile names a Redis URL")
            };
            let (development, test) = (redis(Environment::Development), redis(Environment::Test));

            assert_ne!(
                database(&test),
                database(&development),
                "the test profile ({test}) enqueues on development's database ({development})"
            );
            Ok(())
        });
    }

    /// The suites file into a bucket of their own, beside development's, on
    /// the store `compose.yml` runs, which creates both.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail's closure signature is fixed"
    )]
    fn the_generated_test_profile_files_into_a_bucket_development_never_lists() {
        let prefix = nest_rs_core::EnvPrefix::current();
        figment::Jail::expect_with(|jail| {
            write_profiles(jail.directory(), prefix);
            let mut storage = |profile, key| {
                jail.clear_env();
                load_cascade(jail.directory(), profile);
                ConfigService::for_namespace("storage")
                    .get(key)
                    .expect("the storage setting reads")
                    .expect("the profile names it")
            };
            let development = storage(Environment::Development, "URL");
            let test = storage(Environment::Test, "URL");

            assert_eq!(development, "s3://shop");
            assert_eq!(test, "s3://shop-test");
            Ok(())
        });
    }

    /// The ports a Compose service publishes on the host, from its `ports`
    /// list's `host:container` pairs.
    fn published(service: &yaml_serde::Value) -> Vec<(u16, u16)> {
        service["ports"]
            .as_sequence()
            .expect("the service publishes ports")
            .iter()
            .map(|pair| {
                let (host, container) = pair
                    .as_str()
                    .and_then(|pair| pair.split_once(':'))
                    .expect("a `host:container` pair");
                (
                    host.parse().expect("a host port"),
                    container.parse().expect("a container port"),
                )
            })
            .collect()
    }

    /// A Compose service's `environment` entry `key`.
    fn set<'a>(service: &'a yaml_serde::Value, key: &str) -> &'a str {
        service["environment"][key]
            .as_str()
            .unwrap_or_else(|| panic!("the service sets {key}"))
    }

    /// What each profile boots on is what `compose.yml` runs, read as Compose
    /// reads the file: every URL, endpoint and credential the development and
    /// test profiles name reaches a service it declares, on a port it publishes,
    /// with the account it creates, in a bucket it creates.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail's closure signature is fixed"
    )]
    fn every_service_a_profile_boots_on_is_one_compose_runs() {
        let prefix = nest_rs_core::EnvPrefix::current();
        figment::Jail::expect_with(|jail| {
            let rendering = write_profiles(jail.directory(), prefix);
            let compose: yaml_serde::Value =
                yaml_serde::from_str(&rendering.render(crate::templates::shared::COMPOSE))
                    .expect("compose.yml is YAML");
            let services = &compose["services"];
            let (postgres, redis, storage) = (
                &services["postgres"],
                &services["redis"],
                &services["storage"],
            );
            let buckets = &services["storage-buckets"];
            let script = buckets["command"][0].as_str().expect("the bucket script");
            let created: Vec<&str> = script
                .lines()
                .find_map(|line| line.trim().strip_prefix("for bucket in "))
                .and_then(|list| list.strip_suffix("; do"))
                .expect("the buckets the script creates")
                .split_whitespace()
                .collect();
            let listening: u16 = set(storage, "RUSTFS_ADDRESS")
                .rsplit_once(':')
                .and_then(|(_, port)| port.parse().ok())
                .expect("the store's listening port");
            assert!(
                published(storage)
                    .iter()
                    .any(|(_, inside)| *inside == listening),
                "the store publishes the port it listens on"
            );
            assert!(
                script.contains(&format!(
                    "--user '{}:{}'",
                    set(storage, "RUSTFS_ACCESS_KEY"),
                    set(storage, "RUSTFS_SECRET_KEY")
                )) && script.contains(&format!("http://storage:{listening}/")),
                "the bucket script signs as the store's account, at its port: {script}"
            );
            assert_eq!(
                buckets["depends_on"]["storage"]["condition"].as_str(),
                Some("service_healthy")
            );

            for profile in [Environment::Development, Environment::Test] {
                jail.clear_env();
                load_cascade(jail.directory(), profile);
                let read = |namespace: &str, key: &str| {
                    ConfigService::for_namespace(namespace)
                        .get(key)
                        .expect("the setting reads")
                        .unwrap_or_else(|| panic!("{profile:?} names {namespace} {key}"))
                };
                let url = |namespace, key| {
                    url::Url::parse(&read(namespace, key)).expect("the setting is a URL")
                };
                let on_compose = |url: &url::Url, service: &yaml_serde::Value| {
                    assert_eq!(url.host_str(), Some("localhost"), "{profile:?}: {url}");
                    let port = url.port_or_known_default().expect("a port");
                    assert!(
                        published(service).iter().any(|(host, _)| *host == port),
                        "{profile:?}: {url} on a port compose publishes"
                    );
                };

                let database = url("seaorm", "URL");
                on_compose(&database, postgres);
                assert_eq!(database.username(), set(postgres, "POSTGRES_USER"));
                assert_eq!(
                    database.password(),
                    Some(set(postgres, "POSTGRES_PASSWORD"))
                );
                assert_eq!(&database.path()[1..], set(postgres, "POSTGRES_DB"));

                on_compose(&url("redis", "URL"), redis);

                on_compose(&url("storage", "ENDPOINT"), storage);
                assert_eq!(
                    read("storage", "ACCESS_KEY"),
                    set(storage, "RUSTFS_ACCESS_KEY")
                );
                assert_eq!(
                    read("storage", "SECRET_KEY"),
                    set(storage, "RUSTFS_SECRET_KEY")
                );
                let bucket = url("storage", "URL");
                assert!(
                    bucket
                        .host_str()
                        .is_some_and(|bucket| created.contains(&bucket)),
                    "{profile:?}: {bucket} is a bucket compose creates, among {created:?}"
                );
            }
            Ok(())
        });
    }

    /// Every service `compose.yml` runs is pinned to a release, never a moving
    /// `latest`, and the bucket service runs the store's own image.
    #[test]
    fn compose_runs_each_service_on_a_release_and_the_buckets_on_the_store_s_image() {
        let compose: yaml_serde::Value = yaml_serde::from_str(
            &Renderer::new(&Names::parse("shop")).render(crate::templates::shared::COMPOSE),
        )
        .expect("compose.yml is YAML");
        let services = compose["services"]
            .as_mapping()
            .expect("compose.yml declares its services");
        for (name, service) in services {
            let name = name.as_str().expect("a service's name");
            let image = service["image"]
                .as_str()
                .unwrap_or_else(|| panic!("{name} names its image"));
            assert!(
                image.rsplit_once(':').is_some_and(|(_, tag)| tag
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))),
                "{name} runs {image}, which names no release"
            );
        }
        assert_eq!(
            services["storage-buckets"]["image"], services["storage"]["image"],
            "the bucket service runs the store's image"
        );
    }

    /// Whichever layout renders it, the controller mounts `/` and declares its
    /// posture.
    #[test]
    fn the_shared_hello_controller_mounts_root_as_public() {
        assert!(hello::CONTROLLER.contains(r#"#[controller(path = "/")]"#));
        assert!(hello::CONTROLLER.contains(r#"#[get("/")]"#));
        assert!(hello::CONTROLLER.contains("#[public]"));
    }

    /// The app must actually reach it: the app module imports the feature's
    /// HTTP module, which lists the controller as a provider, whose decorator
    /// brings the transport serving it, so the root names no transport.
    #[test]
    fn the_app_wires_the_hello_controller_in() {
        assert!(workspace::APP_MODULE.contains("{{http_module}},"));
        assert!(!workspace::APP_MODULE.contains("HttpModule::"));
        assert!(hello::FEATURE_HTTP_MODULE.contains("providers = [{{controller}}]"));
        assert!(hello::FEATURE_HTTP_MODULE.contains("imports = [{{module}}]"));
    }
}
