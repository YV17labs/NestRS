use std::time::Duration;

use crate::StorageTls;
use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, DurationBounds, Environment, Floor, Setting, config,
};

/// How long a call waits for S3's answer by default: under the authentication
/// guard's 20-second net and the HTTP edge's 30-second request timeout.
const DEFAULT_OPERATION_TIMEOUT_SECS: u64 = 15;

/// How long a transfer waits for its next bytes by default: the 30 seconds
/// object_store waits for a request by default.
const DEFAULT_READ_TIMEOUT_SECS: u64 = 30;

/// The pair the dev container's RustFS accepts: the base of a development or
/// test profile's [`Config::defaults`], never of `Default`.
const DEV_ACCESS_KEY: &str = "nestrs";
const DEV_SECRET_KEY: &str = "nestrs";

/// The operation budget's range, the variable that sets it, and why.
pub(crate) const OPERATION_TIMEOUT: DurationBounds = DurationBounds::secs(
    "OPERATION_TIMEOUT_SECS",
    "StorageConfig::operation_timeout",
    Floor::AboveZero("a zero budget gives up on every call before S3 is asked"),
    Bound {
        count: 60 * 60,
        why: "the budget bounds every call a caller waits on, and an S3 silent for an hour is \
              gone rather than slow",
    },
);

/// The read bound's range, the variable that sets it, and why.
pub(crate) const READ_TIMEOUT: DurationBounds = DurationBounds::secs(
    "READ_TIMEOUT_SECS",
    "StorageConfig::read_timeout",
    Floor::AboveZero("a zero bound cuts every transfer before its first bytes"),
    Bound {
        count: 60 * 60,
        why: "a transfer that moved nothing for an hour is a dead connection, not a slow one",
    },
);

/// S3-compatible object storage configuration, read from the
/// framework-namespaced `<PREFIX>_STORAGE__*` keys.
///
/// `Default` is the production-safe value: an `https://` endpoint in
/// path-style addressing, plain HTTP refused and no credential. A development
/// or test profile's [`Config::defaults`] adds the dev container's pair and
/// allows plain HTTP; a config pinned over `..Default::default()` gets neither
/// in any profile, and one pinned over `..StorageConfig::defaults()` gets the
/// profile's. For real AWS S3, leave [`endpoint`](Self::endpoint) empty and
/// set [`force_path_style`](Self::force_path_style) to `false`.
#[config(namespace = "storage")]
#[derive(Clone)]
pub struct StorageConfig {
    /// S3 endpoint URL (e.g. `https://rustfs:9000`). Empty ⇒ real AWS S3.
    pub endpoint: String,
    /// The S3 region (required).
    #[validate(length(min = 1, message = "must not be empty"))]
    pub region: String,
    /// The access key id for S3 authentication.
    pub access_key: String,
    /// The secret access key for S3 authentication.
    pub secret_key: String,
    /// The bucket every operation is scoped to (required).
    #[validate(length(min = 1, message = "must not be empty"))]
    pub bucket: String,
    /// `true` ⇒ path-style addressing (`endpoint/bucket/key`), required by
    /// most S3-compatible servers. `false` ⇒ virtual-hosted-style
    /// (`bucket.endpoint/key`), the AWS default.
    pub force_path_style: bool,
    /// Allow reaching the endpoint over plain `http://`
    /// (`<PREFIX>_STORAGE__ALLOW_HTTP`): `false` by default, and `true` only in
    /// a development or test profile's [`Config::defaults`].
    pub allow_http: bool,
    /// The most a call waits for S3's answer, every retry included; a
    /// download's body is held by [`read_timeout`](Self::read_timeout) instead.
    /// Read from `<PREFIX>_STORAGE__OPERATION_TIMEOUT_SECS`, whole seconds from 1
    /// to 3600, and in code anything above zero up to an hour; defaults to 15s.
    /// The boot refuses it at or past a net reaching the client.
    pub operation_timeout: Duration,
    /// The most a download waits for its next bytes while it is read: one that
    /// stalls longer is resumed from where it stopped, and fails naming this
    /// bound if the resumed body sends nothing within it. Read from
    /// `<PREFIX>_STORAGE__READ_TIMEOUT_SECS`, whole seconds from 1 to 3600, and in
    /// code anything above zero up to an hour; defaults to 30s.
    pub read_timeout: Duration,
    /// What an `https://` endpoint's certificate must chain to — the system's
    /// store unless `<PREFIX>_STORAGE__TLS_CA_CERT` names an authority. The
    /// certificate is verified either way.
    pub tls: StorageTls,
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConfig")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("bucket", &self.bucket)
            .field("force_path_style", &self.force_path_style)
            .field("allow_http", &self.allow_http)
            .field("operation_timeout", &self.operation_timeout)
            .field("read_timeout", &self.read_timeout)
            .field("tls", &self.tls)
            .finish()
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://rustfs:9000".into(),
            region: "us-east-1".into(),
            access_key: String::new(),
            secret_key: String::new(),
            bucket: "nestrs".into(),
            force_path_style: true,
            allow_http: false,
            operation_timeout: Duration::from_secs(DEFAULT_OPERATION_TIMEOUT_SECS),
            read_timeout: Duration::from_secs(DEFAULT_READ_TIMEOUT_SECS),
            tls: StorageTls::default(),
        }
    }
}

impl Config for StorageConfig {
    /// In a development or test profile, the dev container's pair and plain
    /// HTTP over `Default`. Here rather than in `from_env`, so it never
    /// rewrites a pinned struct.
    fn defaults() -> Self {
        let safe = Self::default();
        if !dev_profile() {
            return safe;
        }
        Self {
            access_key: DEV_ACCESS_KEY.into(),
            secret_key: DEV_SECRET_KEY.into(),
            allow_http: true,
            ..safe
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        let d = base;
        let allow_http = env.flag("ALLOW_HTTP", d.allow_http)?;
        let access_key = env.setting("ACCESS_KEY")?;
        let secret_key = env.setting("SECRET_KEY")?;
        refuse_half_a_credential(env, &d, access_key.as_ref(), secret_key.as_ref())?;
        let endpoint = resolve_endpoint(env, env.setting("ENDPOINT")?, d.endpoint, allow_http)?;
        Ok(Self {
            tls: StorageTls::from_env(env, d.tls, is_plaintext(&endpoint))?,
            endpoint,
            region: env.get("REGION")?.unwrap_or(d.region),
            access_key: resolve_credential(env, "ACCESS_KEY", access_key, d.access_key)?,
            secret_key: resolve_credential(env, "SECRET_KEY", secret_key, d.secret_key)?,
            bucket: env.get("BUCKET")?.unwrap_or(d.bucket),
            force_path_style: env.flag("FORCE_PATH_STYLE", d.force_path_style)?,
            allow_http,
            operation_timeout: OPERATION_TIMEOUT.read(env, d.operation_timeout)?.value,
            read_timeout: READ_TIMEOUT.read(env, d.read_timeout)?.value,
        })
    }
}

/// Refuse a plain-`http://` endpoint when plain HTTP is disallowed:
/// `object_store`'s `with_allow_http` gates transfers only, never presigning.
fn resolve_endpoint(
    env: &ConfigService,
    setting: Option<Setting>,
    base: String,
    allow_http: bool,
) -> nest_rs_config::Result<String> {
    let allow_http_var = env.var_name("ALLOW_HTTP");
    let reason = |endpoint: &dyn std::fmt::Display| {
        format!(
            "plain-http endpoint `{endpoint}` is refused because {allow_http_var} is false (its \
             value in `StorageConfig::default()`, and in `StorageConfig::defaults()` outside a \
             development or test profile) — credentials and presigned URLs would travel \
             unencrypted; use an https:// endpoint, or set {allow_http_var}=true to opt in"
        )
    };
    match setting {
        Some(setting) if is_plaintext(&setting.value) && !allow_http => {
            Err(setting.refuse(reason(&setting.shown())))
        }
        Some(setting) => Ok(setting.value),
        None if is_plaintext(&base) && !allow_http => {
            Err(ConfigError::parse(env.var_name("ENDPOINT"), reason(&base)))
        }
        None => Ok(base),
    }
}

/// Refuse a credential pair the environment set only half of, whatever supplies
/// the other half: the environment replaces the pair whole or not at all.
fn refuse_half_a_credential(
    env: &ConfigService,
    base: &StorageConfig,
    access_key: Option<&Setting>,
    secret_key: Option<&Setting>,
) -> nest_rs_config::Result<()> {
    let (set, other, other_value, built_in_value) = match (access_key, secret_key) {
        (Some(set), None) => (set, "SECRET_KEY", &base.secret_key, DEV_SECRET_KEY),
        (None, Some(set)) => (set, "ACCESS_KEY", &base.access_key, DEV_ACCESS_KEY),
        _ => return Ok(()),
    };
    let (set_var, other_file_var) = (set.var(), env.var_name(&format!("{other}_FILE")));
    let message = if other_value.trim().is_empty() {
        format!("is not set, nor is {other_file_var}, beside {set_var} — set both")
    } else {
        let source = if other_value == built_in_value {
            "the built-in development default"
        } else {
            "the value pinned in code"
        };
        format!(
            "is not set, nor is {other_file_var}, beside {set_var}, so the other half of the pair \
             would be {source} — set {} too, so the pair is replaced whole rather than mixed",
            env.spellings(other),
        )
    };
    Err(ConfigError::parse(env.var_name(other), message))
}

/// Whether `endpoint` addresses the store over unencrypted HTTP.
pub(crate) fn is_plaintext(endpoint: &str) -> bool {
    endpoint
        .trim_start()
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
}

/// `true` in a development or test profile.
fn dev_profile() -> bool {
    !matches!(
        Environment::from_env(),
        Environment::Production | Environment::Staging
    )
}

/// The resolved credential, refusing a blank one under the spelling that set
/// it, or its variable when unset.
fn resolve_credential(
    env: &ConfigService,
    key: &str,
    setting: Option<Setting>,
    base: String,
) -> nest_rs_config::Result<String> {
    match setting {
        Some(setting) if setting.value.trim().is_empty() => {
            Err(setting.refuse("is blank — it must hold the credential"))
        }
        Some(setting) => Ok(setting.value),
        None if base.trim().is_empty() => Err(ConfigError::parse(
            env.var_name(key),
            format!(
                "must be set, inline or through {} — `StorageConfig::defaults()` holds the \
                 development pair in a development or test profile alone, \
                 `StorageConfig::default()` never, and a config pinned in code reads the pair \
                 from the deployment, not `.env`",
                env.var_name(&format!("{key}_FILE"))
            ),
        )),
        None => Ok(base),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_struct_default_opens_nothing_in_any_profile() {
        let d = StorageConfig::default();
        assert!(
            !d.allow_http,
            "plain HTTP is opened by the profile or in sight"
        );
        assert!(
            d.access_key.is_empty() && d.secret_key.is_empty(),
            "a pair anyone can read is no credential"
        );
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn the_development_pair_and_plain_http_come_with_a_development_profile_alone() {
        figment::Jail::expect_with(|jail| {
            for profile in ["development", "test"] {
                jail.set_env(Environment::var_name(), profile);
                let d = <StorageConfig as Config>::defaults();
                assert_eq!(
                    (d.access_key.as_str(), d.secret_key.as_str(), d.allow_http),
                    (DEV_ACCESS_KEY, DEV_SECRET_KEY, true),
                    "{profile}"
                );
            }
            for profile in ["staging", "production"] {
                jail.set_env(Environment::var_name(), profile);
                let d = <StorageConfig as Config>::defaults();
                assert!(
                    d.access_key.is_empty() && d.secret_key.is_empty() && !d.allow_http,
                    "{profile}"
                );
                assert_eq!(
                    format!("{d:?}"),
                    format!("{:?}", StorageConfig::default()),
                    "{profile}"
                );
            }
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn a_pin_over_the_profiles_defaults_keeps_the_pair_in_development_alone() {
        figment::Jail::expect_with(|jail| {
            let pin = || StorageConfig {
                bucket: "acme-media".into(),
                ..StorageConfig::defaults()
            };
            jail.set_env(Environment::var_name(), "development");
            let resolved =
                StorageConfig::from_env(&unset(), pin()).expect("the pair is the profile's");
            assert_eq!(
                (resolved.bucket.as_str(), resolved.access_key.as_str()),
                ("acme-media", DEV_ACCESS_KEY)
            );
            jail.set_env(Environment::var_name(), "production");
            let refused = StorageConfig::from_env(&unset(), pin())
                .expect_err("production has no pair to give")
                .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("storage", "ACCESS_KEY")),
                "{refused}"
            );
            Ok(())
        });
    }

    #[test]
    fn a_plain_http_endpoint_pinned_over_the_struct_default_is_refused_naming_the_opt_in() {
        let refused = StorageConfig::from_env(
            &unset(),
            StorageConfig {
                endpoint: "http://minio:9000".into(),
                ..Default::default()
            },
        )
        .expect_err("the struct default does not open plain HTTP")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("storage", "ALLOW_HTTP")),
            "{refused}"
        );
        assert!(
            refused.contains(
                "its value in `StorageConfig::default()`, and in `StorageConfig::defaults()` \
                 outside a development or test profile"
            ),
            "the sentence says where the false comes from, whatever the profile: {refused}"
        );
    }

    #[test]
    fn a_pin_over_the_struct_default_takes_its_pair_from_the_deployment() {
        let refused = StorageConfig::from_env(
            &unset(),
            StorageConfig {
                bucket: "media".into(),
                ..Default::default()
            },
        )
        .expect_err("the struct default holds no pair")
        .to_string();
        assert!(
            refused.starts_with(&format!(
                "invalid value for {}",
                nest_rs_config::var_name("storage", "ACCESS_KEY")
            )) && refused.contains("StorageConfig::defaults()"),
            "{refused}"
        );
    }

    fn unset() -> ConfigService {
        ConfigService::with_vars("storage", [])
    }

    /// A base holding a pair of its own, for the reads that are about
    /// something else.
    fn paired() -> StorageConfig {
        StorageConfig {
            access_key: "AKIAPINNED".into(),
            secret_key: "pinned-secret".into(),
            ..StorageConfig::default()
        }
    }

    #[test]
    fn credential_blank_aborts_naming_both_its_spellings() {
        // The struct default holds no pair, so an unset variable over it
        // arrives here blank.
        let err = resolve_credential(&unset(), "SECRET_KEY", None, String::new())
            .expect_err("must abort")
            .to_string();
        assert!(
            err.contains(&nest_rs_config::var_name("storage", "SECRET_KEY")),
            "the error names the variable: {err}",
        );
        assert!(
            err.contains(&nest_rs_config::var_name("storage", "SECRET_KEY_FILE")),
            "and its file spelling: {err}",
        );
        assert!(
            resolve_credential(&unset(), "SECRET_KEY", None, "   ".into()).is_err(),
            "whitespace-only is blank too",
        );
    }

    #[test]
    fn credential_set_is_taken_verbatim() {
        let env = ConfigService::with_vars("storage", [("ACCESS_KEY", "AKIAREAL")]);
        let setting = env.setting("ACCESS_KEY").expect("reads");
        assert_eq!(
            resolve_credential(&env, "ACCESS_KEY", setting, String::new()).expect("set ⇒ ok"),
            "AKIAREAL",
        );
    }

    #[test]
    fn a_plain_http_endpoint_is_refused_when_allow_http_is_false() {
        let err = resolve_endpoint(&unset(), None, "http://minio.internal:9000".into(), false)
            .expect_err("plaintext + allow_http=false must abort boot");
        let rendered = err.to_string();
        assert!(
            rendered.contains("ENDPOINT"),
            "the error names the offending variable: {rendered}",
        );
        assert!(
            rendered.contains("ALLOW_HTTP"),
            "and the opt-in that would allow it: {rendered}",
        );
        assert!(
            resolve_endpoint(&unset(), None, "  HTTP://x:9000".into(), false).is_err(),
            "the scheme check must not be defeated by case or leading space",
        );
    }

    #[test]
    fn https_and_the_empty_aws_endpoint_are_always_accepted() {
        for endpoint in ["https://s3.example", "", "https://minio:9000"] {
            assert_eq!(
                resolve_endpoint(&unset(), None, endpoint.into(), false).expect("encrypted ⇒ ok"),
                endpoint,
            );
        }
    }

    #[test]
    fn a_plain_http_endpoint_is_accepted_when_allow_http_is_opted_in() {
        assert_eq!(
            resolve_endpoint(&unset(), None, "http://rustfs:9000".into(), true)
                .expect("opted in ⇒ ok"),
            "http://rustfs:9000",
        );
    }

    #[test]
    fn from_env_refuses_the_production_pairing_end_to_end() {
        let err = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [("ENDPOINT", "http://minio:9000"), ("ALLOW_HTTP", "false")],
            ),
            StorageConfig::default(),
        )
        .expect_err("the resolved config must not carry a plaintext endpoint");
        assert!(err.to_string().contains("ENDPOINT"));
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config_independently() {
        let pinned = StorageConfig {
            bucket: "pinned-bucket".into(),
            access_key: "AKIAPINNED".into(),
            secret_key: "pinned-secret".into(),
            ..Default::default()
        };
        let cfg = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [
                    ("ENDPOINT", "https://s3.example"),
                    ("ACCESS_KEY", "AKIAREAL"),
                    ("SECRET_KEY", "real-secret"),
                ],
            ),
            pinned,
        )
        .expect("a whole pair over a whole pin resolves");
        assert_eq!(cfg.endpoint, "https://s3.example");
        assert_eq!(cfg.access_key, "AKIAREAL");
        assert_eq!(cfg.secret_key, "real-secret", "the pair is replaced whole");
        assert_eq!(
            cfg.bucket, "pinned-bucket",
            "the pin survives where the env is silent"
        );
    }

    #[test]
    fn half_a_credential_over_a_pinned_pair_is_refused_saying_the_other_is_pinned() {
        let pinned = StorageConfig {
            access_key: "AKIAPINNED".into(),
            secret_key: "pinned-secret".into(),
            ..Default::default()
        };
        for (set, missing) in [("ACCESS_KEY", "SECRET_KEY"), ("SECRET_KEY", "ACCESS_KEY")] {
            let err = StorageConfig::from_env(
                &ConfigService::with_vars("storage", [(set, "FROM-DEPLOY")]),
                pinned.clone(),
            )
            .expect_err("a deployment half beside a pinned half is refused");
            let rendered = err.to_string();
            assert!(
                rendered.starts_with(&format!(
                    "invalid value for {}",
                    nest_rs_config::var_name("storage", missing)
                )),
                "names {missing}: {rendered}"
            );
            assert!(rendered.contains("pinned in code"), "{rendered}");
        }
    }

    #[test]
    fn half_a_credential_beside_the_development_pair_is_refused_saying_so() {
        let development = StorageConfig {
            access_key: DEV_ACCESS_KEY.into(),
            secret_key: DEV_SECRET_KEY.into(),
            ..StorageConfig::default()
        };
        for (set, missing) in [("ACCESS_KEY", "SECRET_KEY"), ("SECRET_KEY", "ACCESS_KEY")] {
            let err = StorageConfig::from_env(
                &ConfigService::with_vars("storage", [(set, "from-the-deployment")]),
                development.clone(),
            )
            .expect_err("half a credential beside the development pair is refused")
            .to_string();
            assert!(
                err.starts_with(&format!(
                    "invalid value for {}",
                    nest_rs_config::var_name("storage", missing)
                )) && err.contains("the built-in development default"),
                "names {missing}: {err}"
            );
        }
    }

    #[test]
    fn half_a_credential_beside_the_struct_default_is_refused_asking_for_both() {
        for (set, missing) in [("ACCESS_KEY", "SECRET_KEY"), ("SECRET_KEY", "ACCESS_KEY")] {
            let err = StorageConfig::from_env(
                &ConfigService::with_vars("storage", [(set, "from-the-deployment")]),
                StorageConfig::default(),
            )
            .expect_err("half a credential beside no pair is refused")
            .to_string();
            assert!(
                err.starts_with(&format!(
                    "invalid value for {}",
                    nest_rs_config::var_name("storage", missing)
                )) && err.contains("set both"),
                "names {missing}: {err}"
            );
        }
    }

    #[test]
    fn the_default_operation_budget_sits_below_every_net_a_call_runs_under() {
        let budget = StorageConfig::default().operation_timeout;
        assert!(budget < nest_rs_authn::AUTHENTICATE_TIMEOUT, "{budget:?}");
        let request = nest_rs_http::HttpConfig::default()
            .request_timeout
            .expect("the edge bounds a request by default");
        assert!(budget < request, "{budget:?}");
    }

    #[test]
    fn the_timeouts_read_whole_seconds_over_a_pinned_base() {
        let cfg = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [("OPERATION_TIMEOUT_SECS", "5"), ("READ_TIMEOUT_SECS", "7")],
            ),
            paired(),
        )
        .expect("both in range");
        assert_eq!(
            (cfg.operation_timeout, cfg.read_timeout),
            (Duration::from_secs(5), Duration::from_secs(7))
        );
    }

    #[test]
    fn a_zero_timeout_is_refused_naming_its_variable_or_its_field() {
        let zero_operation = StorageConfig {
            operation_timeout: Duration::ZERO,
            ..paired()
        };
        let zero_read = StorageConfig {
            read_timeout: Duration::ZERO,
            ..paired()
        };
        for (key, field, pinned) in [
            (
                "OPERATION_TIMEOUT_SECS",
                "StorageConfig::operation_timeout",
                zero_operation,
            ),
            (
                "READ_TIMEOUT_SECS",
                "StorageConfig::read_timeout",
                zero_read,
            ),
        ] {
            let read = StorageConfig::from_env(
                &ConfigService::with_vars("storage", [(key, "0")]),
                paired(),
            )
            .expect_err("zero is refused")
            .to_string();
            assert!(
                read.contains(&nest_rs_config::var_name("storage", key)),
                "{read}"
            );

            let pinned = StorageConfig::from_env(&unset(), pinned)
                .expect_err("a zero pin is refused")
                .to_string();
            assert!(pinned.contains(field), "{pinned}");
        }
    }

    /// A file holding `contents`, removed on drop — the value a `_FILE`
    /// variable names.
    struct SecretFile(std::path::PathBuf);

    impl SecretFile {
        fn new(name: &str, contents: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("nest-rs-storage-{name}-{}", std::process::id()));
            std::fs::write(&path, contents).expect("write the fixture");
            Self(path)
        }

        fn path(&self) -> &str {
            self.0.to_str().expect("a UTF-8 path")
        }
    }

    impl Drop for SecretFile {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the test tears down its temp file best-effort"
        )]
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn half_a_credential_given_as_a_file_names_both_spellings() {
        let access = SecretFile::new("access", "AKIA-FROM-FILE\n");
        let err = StorageConfig::from_env(
            &ConfigService::with_vars("storage", [("ACCESS_KEY_FILE", access.path())]),
            StorageConfig::default(),
        )
        .expect_err("half a credential is refused")
        .to_string();
        assert!(
            err.contains(&nest_rs_config::var_name("storage", "ACCESS_KEY_FILE")),
            "names the spelling that was set: {err}"
        );
        assert!(
            err.contains(&nest_rs_config::var_name("storage", "SECRET_KEY_FILE")),
            "and the other half's file spelling: {err}"
        );
        assert!(!err.contains("AKIA-FROM-FILE"), "{err}");
    }

    #[test]
    fn a_plain_http_endpoint_from_a_file_is_refused_without_quoting_it() {
        let endpoint = SecretFile::new("endpoint", "http://user:hunter2@minio:9000\n");
        let err = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [("ENDPOINT_FILE", endpoint.path()), ("ALLOW_HTTP", "false")],
            ),
            StorageConfig::default(),
        )
        .expect_err("plaintext is refused")
        .to_string();
        assert!(
            err.contains(&nest_rs_config::var_name("storage", "ENDPOINT_FILE")),
            "{err}"
        );
        assert!(!err.contains("hunter2"), "{err}");
    }
}
