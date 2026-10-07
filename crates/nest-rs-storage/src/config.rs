use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, DurationBounds, Environment, Floor, Setting, config,
};

/// How long a call waits for S3's answer by default: under the authentication
/// guard's 20-second net and the HTTP edge's 30-second request timeout, so a
/// silent store is said as its own error beneath both.
const DEFAULT_OPERATION_TIMEOUT_SECS: u64 = 15;

/// How long a transfer waits for its next bytes by default: the 30 seconds
/// object_store waits for a request by default.
const DEFAULT_READ_TIMEOUT_SECS: u64 = 30;

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
/// The defaults target a local S3-compatible server over plain HTTP in
/// path-style addressing (the common shape for MinIO / RustFS in a dev
/// container). For real AWS S3, leave [`endpoint`](Self::endpoint) empty and
/// set [`force_path_style`](Self::force_path_style) to `false`.
#[config(namespace = "storage")]
#[derive(Clone)]
pub struct StorageConfig {
    /// S3 endpoint URL (e.g. `http://rustfs:9000`). Empty ⇒ real AWS S3.
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
    /// Allow reaching the endpoint over plain `http://`. Convenient for a local
    /// MinIO / RustFS dev server, but a footgun in production where credentials
    /// would travel unencrypted — so it is **opt-in outside dev/test**
    /// (`<PREFIX>_STORAGE__ALLOW_HTTP`), defaulting to `true` only in dev/test and
    /// `false` in staging/production (STORAGE-ST2).
    pub allow_http: bool,
    /// The most a call waits for S3's answer, every retry included — the AWS
    /// SDK's operation timeout. An upload's body is part of its request, so
    /// [`put_bytes`](crate::Storage::put_bytes) moves within it, and
    /// [`put_stream`](crate::Storage::put_stream) moves each part within it; a
    /// download's body is a transfer, held by [`read_timeout`](Self::read_timeout)
    /// instead. Retries stop once half of it is spent, so a call whose attempts
    /// keep failing ends on S3's error and its cause; past it a call S3 never
    /// answered fails as S3's own error, naming the budget. Read
    /// from `<PREFIX>_STORAGE__OPERATION_TIMEOUT_SECS`, whole seconds from 1 to
    /// 3600, and in code anything above zero up to an hour; defaults to 15s. The
    /// boot refuses it at or past a net reaching the client — the
    /// authentication guard's 20s when its strategy injects [`Storage`](crate::Storage).
    pub operation_timeout: Duration,
    /// The most a download waits for its next bytes while it is read — the AWS
    /// SDK's read timeout: one that stalls longer is cut and resumed from where
    /// it stopped, fenced on the object's `ETag`, and fails naming this bound if
    /// the resumed body sends nothing within it; one that moves, or whose reader
    /// pauses, is never cut. Read from `<PREFIX>_STORAGE__READ_TIMEOUT_SECS`,
    /// whole seconds from 1 to 3600, and in code anything above zero up to an
    /// hour; defaults to 30s.
    pub read_timeout: Duration,
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
            .finish()
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://rustfs:9000".into(),
            region: "us-east-1".into(),
            access_key: "nestrs".into(),
            secret_key: "nestrs".into(),
            bucket: "nestrs".into(),
            force_path_style: true,
            allow_http: true,
            operation_timeout: Duration::from_secs(DEFAULT_OPERATION_TIMEOUT_SECS),
            read_timeout: Duration::from_secs(DEFAULT_READ_TIMEOUT_SECS),
        }
    }
}

impl Config for StorageConfig {
    /// The unpinned baseline is profile-dependent, and both differences are
    /// security ones. Outside dev/test the dev sentinel credentials
    /// (`nestrs`/`nestrs`) are dropped so an unset `<PREFIX>_STORAGE__ACCESS_KEY`
    /// fails boot naming the variable rather than authenticating with a public
    /// default (STORAGE-ST1), and plain-HTTP is off so credentials never travel
    /// unencrypted by omission (STORAGE-ST2). It lives here rather than in
    /// `from_env` so it applies only where it is a default — overlaying it onto a
    /// pinned struct would rewrite a deliberate choice.
    fn defaults() -> Self {
        let d = Self::default();
        if dev_profile() {
            return d;
        }
        Self {
            access_key: String::new(),
            secret_key: String::new(),
            allow_http: false,
            ..d
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        let d = base;
        let allow_http = env.flag("ALLOW_HTTP", d.allow_http)?;
        let access_key = env.setting("ACCESS_KEY")?;
        let secret_key = env.setting("SECRET_KEY")?;
        refuse_half_a_credential(env, &d, access_key.as_ref(), secret_key.as_ref())?;
        Ok(Self {
            endpoint: resolve_endpoint(env, env.setting("ENDPOINT")?, d.endpoint, allow_http)?,
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

/// Refuse a plain-`http://` endpoint when plain HTTP is disallowed.
///
/// `object_store`'s `with_allow_http` only gates the client's own byte
/// transfers, and it does so as an opaque request-time failure. Presigning is a
/// *local* computation, so it was never gated at all: a production app minted
/// working `http://` URLs carrying the SigV4 signature — the exact leak the
/// default exists to prevent, on the flow `/storage/` calls canonical.
///
/// Rejecting the pairing where the config is resolved fixes both halves at
/// once: no unencrypted transfer can be attempted, no plaintext URL can be
/// signed, and a mis-deployed app fails boot naming the variable instead of
/// starting healthy and 500-ing on first use. The refusal names the spelling
/// the deployment set and quotes the endpoint only when no file held it.
fn resolve_endpoint(
    env: &ConfigService,
    setting: Option<Setting>,
    base: String,
    allow_http: bool,
) -> nest_rs_config::Result<String> {
    let allow_http_var = env.var_name("ALLOW_HTTP");
    let reason = |endpoint: &dyn std::fmt::Display| {
        format!(
            "plain-http endpoint `{endpoint}` is refused because {allow_http_var} is false \
             (the staging/production default) — credentials and presigned URLs would travel \
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
/// the other half.
///
/// An access key and its secret are one credential. The environment's half
/// beside a half pinned in code, or beside the public `nestrs` development
/// sentinel, is a pair nobody issued: the store refuses it at first use, or —
/// against a dev server that still accepts the sentinel — the app talks to it
/// with a key nobody chose. So the environment replaces the pair whole or not
/// at all, and the refusal says where the other half would have come from — the
/// shape the HTTP transport's half-pair refusal has over a pinned certificate.
fn refuse_half_a_credential(
    env: &ConfigService,
    base: &StorageConfig,
    access_key: Option<&Setting>,
    secret_key: Option<&Setting>,
) -> nest_rs_config::Result<()> {
    let built_in = StorageConfig::default();
    let (set, other, other_value, built_in_value) = match (access_key, secret_key) {
        (Some(set), None) => (set, "SECRET_KEY", &base.secret_key, &built_in.secret_key),
        (None, Some(set)) => (set, "ACCESS_KEY", &base.access_key, &built_in.access_key),
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
///
/// The single spelling of the rule: both the boot-time refusal here and the
/// client's last-line-of-defence check call it, so a future tweak (an IDN host,
/// a scheme-relative endpoint) cannot leave the two enforcing different rules.
pub(crate) fn is_plaintext(endpoint: &str) -> bool {
    endpoint
        .trim_start()
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
}

/// `true` in every profile but staging/production.
fn dev_profile() -> bool {
    !matches!(
        Environment::from_env(),
        Environment::Production | Environment::Staging
    )
}

/// The resolved credential, refusing a blank one by naming its variable. An
/// unset one is blank only outside dev/test, where [`Config::defaults`] drops
/// the dev sentinel — so this is where STORAGE-ST1 lands as a boot error. A
/// value the deployment set blank is refused under the spelling that set it.
fn resolve_credential(
    env: &ConfigService,
    key: &str,
    setting: Option<Setting>,
    base: String,
) -> nest_rs_config::Result<String> {
    const NO_FALLBACK: &str = "in staging/production (no dev-credential fallback outside dev/test)";
    match setting {
        Some(setting) if setting.value.trim().is_empty() => {
            Err(setting.refuse(format_args!("is blank — it must be set {NO_FALLBACK}")))
        }
        Some(setting) => Ok(setting.value),
        None if base.trim().is_empty() => Err(ConfigError::parse(
            env.var_name(key),
            format!(
                "must be set, inline or through {}, {NO_FALLBACK}",
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
    fn default_allows_http_for_local_dev_servers() {
        assert!(
            StorageConfig::default().allow_http,
            "the dev default targets a plain-http RustFS/MinIO server",
        );
    }

    #[test]
    fn the_struct_default_keeps_the_dev_sentinel_credentials() {
        // `Default` is the pin-friendly value a call site writes
        // `..Default::default()` against; the profile floor lives in `defaults`.
        let d = StorageConfig::default();
        assert_eq!(d.access_key, "nestrs");
        assert_eq!(d.secret_key, "nestrs");
    }

    fn unset() -> ConfigService {
        ConfigService::with_vars("storage", [])
    }

    #[test]
    fn credential_blank_aborts_naming_both_its_spellings() {
        // STORAGE-ST1: outside dev/test `Config::defaults` drops the sentinel, so
        // an unset variable arrives here blank and must abort by name.
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

    // G1/G2: `with_allow_http` only gates the client's own byte transfers, and
    // only as an opaque request-time 500 — presigning is local, so it minted
    // working plaintext URLs in production. The pairing has to die at load.
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
        // Case-insensitive and whitespace-tolerant — a scheme is not a shibboleth.
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
        // The dev/test default, and the documented production opt-in.
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

    // The whole point of the overlay: a pinned bucket must not freeze the
    // credentials or the endpoint alongside it.
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

    /// Half a pair from the environment over a pair pinned in code is refused
    /// too, in both directions — a key the deployment issued beside a secret
    /// someone pinned is not a credential — and the refusal says the other half
    /// is the pinned one.
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

    /// One half of the credential pair from the environment beside the other
    /// half's built-in development default is refused, naming the missing
    /// variable — in both directions.
    #[test]
    fn half_a_credential_beside_the_built_in_default_is_refused() {
        for (set, missing) in [("ACCESS_KEY", "SECRET_KEY"), ("SECRET_KEY", "ACCESS_KEY")] {
            let err = StorageConfig::from_env(
                &ConfigService::with_vars("storage", [(set, "from-the-deployment")]),
                StorageConfig::default(),
            )
            .expect_err("half a credential beside the dev default is refused");
            assert!(
                err.to_string().starts_with(&format!(
                    "invalid value for {}",
                    nest_rs_config::var_name("storage", missing)
                )),
                "names {missing}: {err}"
            );
        }
    }

    /// A call waits on S3 under the authentication guard's net when a strategy
    /// injects the client, and under the HTTP edge's deadline in a handler: the
    /// default budget sits below both defaults, so a silent store answers with
    /// its own error, never theirs.
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
            StorageConfig::default(),
        )
        .expect("both in range");
        assert_eq!(
            (cfg.operation_timeout, cfg.read_timeout),
            (Duration::from_secs(5), Duration::from_secs(7))
        );
    }

    /// Off is not a setting for either: a zero budget gives up before S3 is
    /// asked, a zero read bound cuts every transfer — refused from the
    /// environment by variable, and pinned in code by field.
    #[test]
    fn a_zero_timeout_is_refused_naming_its_variable_or_its_field() {
        let zero_operation = StorageConfig {
            operation_timeout: Duration::ZERO,
            ..StorageConfig::default()
        };
        let zero_read = StorageConfig {
            read_timeout: Duration::ZERO,
            ..StorageConfig::default()
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
                StorageConfig::default(),
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

    /// Half a pair given as a file is refused naming the `_FILE` spelling that
    /// was set, and both spellings of the half that was not.
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

    /// A plaintext endpoint read from a file is refused under its `_FILE`
    /// variable, and the endpoint is not quoted.
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
