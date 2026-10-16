use std::time::Duration;

use nest_rs_config::{
    Bound, ClientTls, Config, ConfigError, ConfigService, DurationBounds, Environment, Floor,
    Namespaced, Setting, config, endpoint_shown,
};

use crate::backend::{Backend, Unopened};

/// How long a call waits for the store's answer by default: under the authentication
/// guard's 18-second net and the HTTP edge's 20-second request timeout.
const DEFAULT_OPERATION_TIMEOUT_SECS: u64 = 15;

/// How long a transfer waits for its next bytes by default: the 30 seconds
/// object_store waits for a request by default.
const DEFAULT_READ_TIMEOUT_SECS: u64 = 30;

/// The dev container's RustFS, its bucket and the pair it accepts: the base of
/// a development or test profile's [`Config::defaults`], never of `Default`.
const DEV_URL: &str = "s3://nestrs";
const DEV_ENDPOINT: &str = "https://rustfs:9000";
const DEV_ACCESS_KEY: &str = "nestrs";
const DEV_SECRET_KEY: &str = "nestrs";

/// The operation budget's range, the variable that sets it, and why.
pub(crate) const OPERATION_TIMEOUT: DurationBounds = DurationBounds::secs(
    "OPERATION_TIMEOUT_SECS",
    "StorageConfig::operation_timeout",
    Floor::AboveZero("a zero budget gives up on every call before the store is asked"),
    Bound {
        count: 60 * 60,
        why: "the budget bounds every call a caller waits on, and a store silent for an hour is \
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

/// The object store's configuration, read from the framework-namespaced
/// `<PREFIX>_STORAGE__*` keys: [`url`](Self::url) names the store, and its
/// scheme picks the backend — `s3` or `memory`.
///
/// `Default` is the production-safe value: no store named, no credential,
/// plain HTTP refused, AWS's own endpoint. A declared development or test
/// profile's [`Config::defaults`] names the dev container's RustFS bucket,
/// its pair, and allows plain HTTP; a config pinned over `..Default::default()`
/// gets none of it in any profile, and one pinned over
/// `..StorageConfig::defaults()` gets the profile's. The S3 settings are read
/// for an `s3://` store alone: set beside `memory:///`, the boot refuses them.
#[config(namespace = "storage")]
#[derive(Clone)]
pub struct StorageConfig {
    /// The store, whose scheme picks the backend: `s3://<bucket>[/<prefix>]`
    /// for AWS S3 or a server speaking its API, `memory:///` for one in this
    /// process — development and tests alone, since a restart loses it. A
    /// prefix confines every key under it.
    pub url: String,
    /// S3 only: an S3-compatible server's endpoint (`https://rustfs:9000`);
    /// `None` ⇒ AWS's own.
    pub endpoint: Option<String>,
    /// S3 only: the region requests are signed for.
    #[validate(length(min = 1, message = "must not be empty"))]
    pub region: String,
    /// S3 only: `true` ⇒ path-style addressing (`endpoint/bucket/key`), which
    /// most S3-compatible servers require; `false` ⇒ virtual-hosted-style
    /// (`bucket.endpoint/key`), AWS's.
    pub force_path_style: bool,
    /// S3 only: the access key id requests are signed with.
    pub access_key: String,
    /// S3 only: the secret access key requests are signed with.
    pub secret_key: String,
    /// S3 only: allow reaching the endpoint over plain `http://`
    /// (`<PREFIX>_STORAGE__ALLOW_HTTP`): `false` by default, and `true` only in
    /// a declared development or test profile's [`Config::defaults`].
    pub allow_http: bool,
    /// The most a call waits for the store's answer, every retry included,
    /// and the boot for its proof; a download's body is held by
    /// [`read_timeout`](Self::read_timeout) instead. Read from
    /// `<PREFIX>_STORAGE__OPERATION_TIMEOUT_SECS`, whole seconds from 1 to
    /// 3600, and in code anything above zero up to an hour; defaults to 15s.
    /// The boot refuses it at or past a net reaching the client.
    pub operation_timeout: Duration,
    /// The most a download waits for its next bytes while it is read: one that
    /// stalls longer is resumed from where it stopped, and fails naming this
    /// bound if the resumed body sends nothing within it. Read from
    /// `<PREFIX>_STORAGE__READ_TIMEOUT_SECS`, whole seconds from 1 to 3600, and in
    /// code anything above zero up to an hour; defaults to 30s.
    pub read_timeout: Duration,
    /// S3 only: what an `https://` endpoint's certificate must chain to — the
    /// system's authorities unless `<PREFIX>_STORAGE__TLS_CA_CERT` names others,
    /// which replace them. The certificate is verified either way, and its name
    /// checked against the endpoint's host. A client certificate
    /// (`<PREFIX>_STORAGE__TLS_CERT` and `TLS_KEY`) is refused: object_store
    /// cannot present one.
    pub tls: ClientTls,
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageConfig")
            .field("url", &endpoint_shown(&self.url))
            .field("endpoint", &self.endpoint.as_deref().map(endpoint_shown))
            .field("region", &self.region)
            .field("force_path_style", &self.force_path_style)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
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
            url: String::new(),
            endpoint: None,
            region: "us-east-1".into(),
            force_path_style: true,
            access_key: String::new(),
            secret_key: String::new(),
            allow_http: false,
            operation_timeout: Duration::from_secs(DEFAULT_OPERATION_TIMEOUT_SECS),
            read_timeout: Duration::from_secs(DEFAULT_READ_TIMEOUT_SECS),
            tls: ClientTls::default(),
        }
    }
}

impl Config for StorageConfig {
    /// In a declared development or test profile, the dev container's RustFS
    /// bucket, its pair and plain HTTP over `Default`. Here rather than in
    /// `from_env`, so it never rewrites a pinned struct.
    fn defaults() -> Self {
        let safe = Self::default();
        if Environment::from_env().is_deployed() {
            return safe;
        }
        Self {
            url: DEV_URL.into(),
            endpoint: Some(DEV_ENDPOINT.into()),
            access_key: DEV_ACCESS_KEY.into(),
            secret_key: DEV_SECRET_KEY.into(),
            allow_http: true,
            ..safe
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
        let d = base;
        let url = env.setting("URL")?;
        let backend = match &url {
            Some(setting) => Backend::parse(&setting.value).map_err(|why| setting.refuse(why)),
            None => Backend::parse(&d.url).map_err(|why| refuse_url(env, why)),
        }?;
        let url = url.map_or(d.url.clone(), |setting| setting.value);
        let operation_timeout = OPERATION_TIMEOUT.read(env, d.operation_timeout)?.value;
        let read_timeout = READ_TIMEOUT.read(env, d.read_timeout)?.value;
        if let Backend::Memory { .. } = backend {
            refuse_memory_deployed(env)?;
            refuse_s3_settings(env)?;
            return Ok(Self {
                url,
                operation_timeout,
                read_timeout,
                ..d
            });
        }
        let allow_http = env.flag("ALLOW_HTTP", d.allow_http)?;
        let access_key = env.setting("ACCESS_KEY")?;
        let secret_key = env.setting("SECRET_KEY")?;
        refuse_half_a_credential(env, &d, access_key.as_ref(), secret_key.as_ref())?;
        let endpoint = resolve_endpoint(env, env.setting("ENDPOINT")?, d.endpoint, allow_http)?;
        let tls = ClientTls::from_env(env, d.tls)?;
        refuse_tls(env, endpoint.as_deref(), &tls)?;
        Ok(Self {
            url,
            tls,
            endpoint,
            region: env.get("REGION")?.unwrap_or(d.region),
            access_key: resolve_credential("ACCESS_KEY", access_key, d.access_key)?,
            secret_key: resolve_credential("SECRET_KEY", secret_key, d.secret_key)?,
            force_path_style: env.flag("FORCE_PATH_STYLE", d.force_path_style)?,
            allow_http,
            operation_timeout,
            read_timeout,
        })
    }
}

impl StorageConfig {
    /// The store this config names, once every refusal the read makes holds
    /// for a config built in code too — what [`Storage::connect`] checks
    /// before anything is sent.
    ///
    /// [`Storage::connect`]: crate::Storage::connect
    pub(crate) fn backend(&self) -> nest_rs_config::Result<Backend> {
        let env = ConfigService::for_namespace(Self::NAMESPACE);
        let backend = Backend::parse(&self.url).map_err(|why| refuse_url(&env, why))?;
        for (bounds, value) in [
            (OPERATION_TIMEOUT, self.operation_timeout),
            (READ_TIMEOUT, self.read_timeout),
        ] {
            bounds.check(Self::NAMESPACE, bounds.field(), value)?;
        }
        if let Backend::Memory { .. } = backend {
            refuse_memory_deployed(&env)?;
            return Ok(backend);
        }
        refuse_tls(&env, self.endpoint.as_deref(), &self.tls)?;
        for (key, value) in [
            ("ACCESS_KEY", &self.access_key),
            ("SECRET_KEY", &self.secret_key),
        ] {
            if value.trim().is_empty() {
                return Err(ConfigError::unset(Self::NAMESPACE, key, PAIR_WHY));
            }
        }
        Ok(backend)
    }
}

/// A URL that names no store, refused naming the variable, never quoting it.
fn refuse_url(env: &ConfigService, why: Unopened) -> ConfigError {
    ConfigError::parse(env.var_name("URL"), why.to_string())
}

/// Refuse the in-process store outside a declared development or test
/// profile: it loses every object at a restart.
fn refuse_memory_deployed(env: &ConfigService) -> nest_rs_config::Result<()> {
    let deployed = Environment::from_env();
    if !deployed.is_deployed() {
        return Ok(());
    }
    Err(ConfigError::parse(
        env.var_name("URL"),
        format!(
            "names memory, a store in this process that loses every object at a restart — \
             refused in a {} profile, and in one {} does not declare: name an s3:// store",
            deployed.as_str(),
            Environment::var_name(),
        ),
    ))
}

/// The settings an `s3://` store reads and the in-process one does not.
const S3_ONLY: [&str; 9] = [
    "ENDPOINT",
    "REGION",
    "FORCE_PATH_STYLE",
    "ACCESS_KEY",
    "SECRET_KEY",
    "ALLOW_HTTP",
    "TLS_CA_CERT",
    "TLS_CERT",
    "TLS_KEY",
];

/// Refuse an S3 setting the deployment set beside `memory:///`, which would
/// read none of it, naming the first one set.
fn refuse_s3_settings(env: &ConfigService) -> nest_rs_config::Result<()> {
    for key in S3_ONLY {
        if let Some(set) = env.setting(key)? {
            return Err(set.refuse(format!(
                "is set beside {}, which names memory, a store that reads no S3 setting — \
                 unset it, or name an s3:// store",
                env.var_name("URL"),
            )));
        }
    }
    Ok(())
}

/// Refuse a plain-`http://` endpoint when plain HTTP is disallowed:
/// `object_store`'s `with_allow_http` gates transfers only, never presigning.
/// A blank endpoint is AWS's own.
fn resolve_endpoint(
    env: &ConfigService,
    setting: Option<Setting>,
    base: Option<String>,
    allow_http: bool,
) -> nest_rs_config::Result<Option<String>> {
    let allow_http_var = env.var_name("ALLOW_HTTP");
    let reason = |endpoint: &str| {
        format!(
            "the plain-http endpoint {} is refused because {allow_http_var} is false (its \
             value in `StorageConfig::default()`, and in `StorageConfig::defaults()` outside a \
             declared development or test profile) — credentials and presigned URLs would travel \
             unencrypted; use an https:// endpoint, or set {allow_http_var}=true to opt in",
            endpoint_shown(endpoint)
        )
    };
    match setting {
        Some(setting) if setting.value.trim().is_empty() => Ok(None),
        Some(setting) if is_plaintext(&setting.value) && !allow_http => {
            Err(setting.refuse(reason(&setting.value)))
        }
        Some(setting) => Ok(Some(setting.value)),
        None => match base {
            Some(base) if is_plaintext(&base) && !allow_http => {
                Err(ConfigError::parse(env.var_name("ENDPOINT"), reason(&base)))
            }
            base => Ok(base.filter(|endpoint| !endpoint.trim().is_empty())),
        },
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

/// The library the client reaches S3 through, as a refusal names it.
const OBJECT_STORE: &str = "object_store, the library storage reaches S3 through,";

/// Refuse TLS material the client cannot use: any beside a plain-http
/// endpoint, a client certificate object_store cannot present, and material no
/// handshake could use — at the read, and for a config built in code when the
/// client is opened.
fn refuse_tls(
    env: &ConfigService,
    endpoint: Option<&str>,
    tls: &ClientTls,
) -> nest_rs_config::Result<()> {
    if endpoint.is_some_and(is_plaintext) {
        tls.refuse_beside_plaintext(env, &env.var_name("ENDPOINT"))?;
    }
    tls.refuse_identity(env, OBJECT_STORE)?;
    tls.check(env)
}

/// Whether `endpoint` addresses the store over unencrypted HTTP.
pub(crate) fn is_plaintext(endpoint: &str) -> bool {
    endpoint
        .trim_start()
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
}

/// Why an unset credential is refused rather than defaulted.
const PAIR_WHY: &str = "`StorageConfig::defaults()` holds the development pair in a declared \
                        development or test profile alone, `StorageConfig::default()` never, and \
                        a config pinned in code reads the pair from the deployment, not `.env`";

/// The resolved credential, refusing a blank one under the spelling that set
/// it, or its variable when unset.
fn resolve_credential(
    key: &str,
    setting: Option<Setting>,
    base: String,
) -> nest_rs_config::Result<String> {
    match setting {
        Some(setting) if setting.value.trim().is_empty() => {
            Err(setting.refuse("is blank — it must hold the credential"))
        }
        Some(setting) => Ok(setting.value),
        None if base.trim().is_empty() => {
            Err(ConfigError::unset(StorageConfig::NAMESPACE, key, PAIR_WHY))
        }
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
        assert!(
            d.url.is_empty() && d.endpoint.is_none(),
            "no development server is named outside a declared profile"
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
                assert_eq!(
                    (d.url.as_str(), d.endpoint.as_deref()),
                    (DEV_URL, Some(DEV_ENDPOINT)),
                    "{profile}"
                );
            }
            for profile in ["staging", "production", ""] {
                jail.set_env(Environment::var_name(), profile);
                let d = <StorageConfig as Config>::defaults();
                assert!(
                    d.access_key.is_empty() && d.secret_key.is_empty() && !d.allow_http,
                    "{profile:?}"
                );
                assert_eq!(
                    format!("{d:?}"),
                    format!("{:?}", StorageConfig::default()),
                    "{profile:?}"
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
                url: "s3://acme-media".into(),
                ..StorageConfig::defaults()
            };
            jail.set_env(Environment::var_name(), "development");
            let resolved =
                StorageConfig::from_env(&unset(), pin()).expect("the pair is the profile's");
            assert_eq!(
                (resolved.url.as_str(), resolved.access_key.as_str()),
                ("s3://acme-media", DEV_ACCESS_KEY)
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
                endpoint: Some("http://minio:9000".into()),
                ..s3()
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
                 outside a declared development or test profile"
            ),
            "the sentence says where the false comes from, whatever the profile: {refused}"
        );
    }

    #[test]
    fn a_pin_over_the_struct_default_takes_its_pair_from_the_deployment() {
        let refused = StorageConfig::from_env(&unset(), s3())
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

    /// The struct default naming an S3 bucket, for the reads about what
    /// stands beside it.
    fn s3() -> StorageConfig {
        StorageConfig {
            url: "s3://media".into(),
            ..StorageConfig::default()
        }
    }

    #[test]
    fn an_authority_is_read_for_an_encrypted_endpoint() {
        let authority = nest_rs_testing::TestAuthority::new();
        let tls = StorageConfig::from_env(
            &ConfigService::with_vars("storage", [("TLS_CA_CERT", authority.pem())]),
            paired(),
        )
        .expect("an authority resolves")
        .tls;
        assert_eq!(tls.authorities_pem(), authority.pem().as_bytes());
        assert!(
            StorageConfig::from_env(&unset(), paired())
                .expect("nothing set resolves")
                .tls
                .is_empty(),
            "and nothing set trusts the system's authorities"
        );
    }

    #[test]
    fn an_authority_that_cannot_be_used_is_refused_naming_its_variable() {
        let authority = nest_rs_testing::TestAuthority::new();
        let var = nest_rs_config::var_name("storage", "TLS_CA_CERT");
        let beside_plaintext = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [
                    ("TLS_CA_CERT", authority.pem()),
                    ("ENDPOINT", "http://rustfs:9000"),
                    ("ALLOW_HTTP", "true"),
                ],
            ),
            paired(),
        )
        .expect_err("an authority for a plaintext endpoint")
        .to_string();
        assert!(
            beside_plaintext.contains(&var)
                && beside_plaintext.contains(&nest_rs_config::var_name("storage", "ENDPOINT"))
                && beside_plaintext.contains("plaintext"),
            "{beside_plaintext}"
        );
        let empty = StorageConfig::from_env(
            &ConfigService::with_vars("storage", [("TLS_CA_CERT", "not a certificate s3cret")]),
            paired(),
        )
        .expect_err("an authority holding no certificate")
        .to_string();
        assert!(
            empty.contains(&var)
                && empty.contains("no CERTIFICATE block")
                && !empty.contains("s3cret"),
            "{empty}"
        );
    }

    /// object_store has no client-certificate setting, so a certificate the
    /// deployment sets would go unused while the store refuses the client.
    #[test]
    fn a_client_certificate_is_refused_naming_object_store_and_its_variable() {
        let issued = nest_rs_testing::TestAuthority::new().client("nestrs-test-client");
        let refused = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [
                    ("TLS_CERT", issued.cert.as_str()),
                    ("TLS_KEY", issued.key.as_str()),
                ],
            ),
            paired(),
        )
        .expect_err("a certificate object_store cannot present")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("storage", "TLS_CERT"))
                && refused.contains("object_store"),
            "{refused}"
        );
    }

    /// A base holding a pair of its own, for the reads that are about
    /// something else.
    fn paired() -> StorageConfig {
        StorageConfig {
            access_key: "AKIAPINNED".into(),
            secret_key: "pinned-secret".into(),
            ..s3()
        }
    }

    #[test]
    fn credential_blank_aborts_naming_both_its_spellings() {
        // The struct default holds no pair, so an unset variable over it
        // arrives here blank.
        let err = resolve_credential("SECRET_KEY", None, String::new())
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
            resolve_credential("SECRET_KEY", None, "   ".into()).is_err(),
            "whitespace-only is blank too",
        );
    }

    #[test]
    fn credential_set_is_taken_verbatim() {
        let env = ConfigService::with_vars("storage", [("ACCESS_KEY", "AKIAREAL")]);
        let setting = env.setting("ACCESS_KEY").expect("reads");
        assert_eq!(
            resolve_credential("ACCESS_KEY", setting, String::new()).expect("set ⇒ ok"),
            "AKIAREAL",
        );
    }

    #[test]
    fn a_plain_http_endpoint_is_refused_when_allow_http_is_false() {
        let err = resolve_endpoint(
            &unset(),
            None,
            Some("http://minio.internal:9000".into()),
            false,
        )
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
            resolve_endpoint(&unset(), None, Some("  HTTP://x:9000".into()), false).is_err(),
            "the scheme check must not be defeated by case or leading space",
        );
    }

    /// The endpoint may carry a credential in its userinfo: the refusal shows
    /// it through `endpoint_shown`, as `Debug` does.
    #[test]
    fn a_refused_endpoint_is_shown_without_its_userinfo() {
        for (setting, base) in [
            (
                ConfigService::with_vars("storage", [("ENDPOINT", "http://k:s3cr3t@minio:9000")])
                    .setting("ENDPOINT")
                    .expect("reads"),
                None,
            ),
            (None, Some("http://k:s3cr3t@minio:9000".to_owned())),
        ] {
            let refused = resolve_endpoint(&unset(), setting, base, false)
                .expect_err("plaintext + allow_http=false")
                .to_string();
            assert!(refused.contains("http://minio:9000/"), "{refused}");
            assert!(!refused.contains("s3cr3t"), "{refused}");
        }
        let pinned = StorageConfig {
            url: "s3://bucket/a?token=s3cr3t".into(),
            endpoint: Some("https://k:s3cr3t@minio:9000".into()),
            ..StorageConfig::default()
        };
        assert!(!format!("{pinned:?}").contains("s3cr3t"), "{pinned:?}");
    }

    #[test]
    fn https_and_the_unset_aws_endpoint_are_always_accepted() {
        for endpoint in [Some("https://s3.example"), None, Some("https://minio:9000")] {
            assert_eq!(
                resolve_endpoint(&unset(), None, endpoint.map(str::to_owned), false)
                    .expect("encrypted ⇒ ok")
                    .as_deref(),
                endpoint,
            );
        }
        let blank = ConfigService::with_vars("storage", [("ENDPOINT", " ")]);
        assert_eq!(
            resolve_endpoint(
                &blank,
                blank.setting("ENDPOINT").expect("reads"),
                None,
                false
            )
            .expect("a blank endpoint is AWS's own"),
            None
        );
    }

    #[test]
    fn a_plain_http_endpoint_is_accepted_when_allow_http_is_opted_in() {
        assert_eq!(
            resolve_endpoint(&unset(), None, Some("http://rustfs:9000".into()), true)
                .expect("opted in ⇒ ok")
                .as_deref(),
            Some("http://rustfs:9000"),
        );
    }

    #[test]
    fn from_env_refuses_the_production_pairing_end_to_end() {
        let err = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [("ENDPOINT", "http://minio:9000"), ("ALLOW_HTTP", "false")],
            ),
            s3(),
        )
        .expect_err("the resolved config must not carry a plaintext endpoint");
        assert!(err.to_string().contains("ENDPOINT"));
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config_independently() {
        let pinned = StorageConfig {
            url: "s3://pinned-bucket".into(),
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
        assert_eq!(cfg.endpoint.as_deref(), Some("https://s3.example"));
        assert_eq!(cfg.access_key, "AKIAREAL");
        assert_eq!(cfg.secret_key, "real-secret", "the pair is replaced whole");
        assert_eq!(
            cfg.url, "s3://pinned-bucket",
            "the pin survives where the env is silent"
        );
    }

    #[test]
    fn half_a_credential_over_a_pinned_pair_is_refused_saying_the_other_is_pinned() {
        let pinned = paired();
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
            ..s3()
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
                s3(),
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
            s3(),
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
            s3(),
        )
        .expect_err("plaintext is refused")
        .to_string();
        assert!(
            err.contains(&nest_rs_config::var_name("storage", "ENDPOINT_FILE")),
            "{err}"
        );
        assert!(!err.contains("hunter2"), "{err}");
    }

    #[test]
    fn a_url_naming_no_store_is_refused_naming_its_variable_and_never_quoting_it() {
        let var = nest_rs_config::var_name("storage", "URL");
        for (url, says) in [
            (
                "file:///var/s3cr3t",
                "names `file`, which this release does not open",
            ),
            ("gs://s3cr3t", "names `gs`"),
            ("s3://key:s3cr3t@bucket", "a credential"),
        ] {
            let read = StorageConfig::from_env(
                &ConfigService::with_vars("storage", [("URL", url)]),
                paired(),
            )
            .expect_err("no store this release opens")
            .to_string();
            let pinned = StorageConfig::from_env(
                &unset(),
                StorageConfig {
                    url: url.into(),
                    ..paired()
                },
            )
            .expect_err("no store this release opens, pinned")
            .to_string();
            for refused in [read, pinned] {
                assert!(
                    refused.contains(&var) && refused.contains(says),
                    "{url}: {refused}"
                );
                assert!(!refused.contains("s3cr3t"), "{refused}");
            }
        }
    }

    #[test]
    fn the_struct_default_names_no_store_and_the_boot_says_which_variable() {
        let refused = StorageConfig::from_env(&unset(), StorageConfig::default())
            .expect_err("no store is named")
            .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("storage", "URL"))
                && refused.contains("is not set"),
            "{refused}"
        );
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn memory_opens_in_a_development_or_test_profile_and_is_refused_elsewhere() {
        figment::Jail::expect_with(|jail| {
            let memory = ConfigService::with_vars("storage", [("URL", "memory:///")]);
            for profile in ["development", "test"] {
                jail.set_env(Environment::var_name(), profile);
                let resolved = StorageConfig::from_env(&memory, StorageConfig::default())
                    .expect("memory needs no S3 setting");
                assert_eq!(resolved.url, "memory:///", "{profile}");
                assert!(
                    StorageConfig {
                        url: "memory:///".into(),
                        ..StorageConfig::default()
                    }
                    .backend()
                    .is_ok(),
                    "{profile}"
                );
            }
            for profile in ["staging", "production", ""] {
                jail.set_env(Environment::var_name(), profile);
                let read = StorageConfig::from_env(&memory, StorageConfig::default())
                    .expect_err("memory loses everything at a restart")
                    .to_string();
                let built = StorageConfig {
                    url: "memory:///".into(),
                    ..StorageConfig::default()
                }
                .backend()
                .expect_err("and so does one built in code")
                .to_string();
                for refused in [read, built] {
                    assert!(
                        refused.contains(&nest_rs_config::var_name("storage", "URL"))
                            && refused.contains("loses every object at a restart"),
                        "{profile:?}: {refused}"
                    );
                }
            }
            Ok(())
        });
    }

    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn an_s3_setting_beside_memory_is_refused_naming_what_is_set() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(Environment::var_name(), "test");
            let authority = nest_rs_testing::TestAuthority::new();
            for (key, value) in [
                ("ENDPOINT", "https://rustfs:9000"),
                ("REGION", "eu-west-3"),
                ("FORCE_PATH_STYLE", "false"),
                ("ACCESS_KEY", "AKIA"),
                ("SECRET_KEY", "s3cr3t"),
                ("ALLOW_HTTP", "true"),
                ("TLS_CA_CERT", authority.pem()),
            ] {
                let refused = StorageConfig::from_env(
                    &ConfigService::with_vars("storage", [("URL", "memory:///"), (key, value)]),
                    StorageConfig::default(),
                )
                .expect_err("memory reads no S3 setting")
                .to_string();
                assert!(
                    refused.starts_with(&format!(
                        "invalid value for {}",
                        nest_rs_config::var_name("storage", key)
                    )) && refused.contains(&nest_rs_config::var_name("storage", "URL")),
                    "{key}: {refused}"
                );
                assert!(!refused.contains("s3cr3t"), "{refused}");
            }
            Ok(())
        });
    }

    #[test]
    fn a_config_built_in_code_is_refused_what_the_read_refuses_naming_the_variables() {
        let blank_pair = StorageConfig {
            access_key: String::new(),
            ..paired()
        }
        .backend()
        .expect_err("a blank half of the pair")
        .to_string();
        assert!(
            blank_pair.contains(&nest_rs_config::var_name("storage", "ACCESS_KEY")),
            "{blank_pair}"
        );
        let unopened = StorageConfig {
            url: "file:///media".into(),
            ..paired()
        }
        .backend()
        .expect_err("a scheme this release does not open")
        .to_string();
        assert!(
            unopened.contains(&nest_rs_config::var_name("storage", "URL"))
                && unopened.contains("`file`"),
            "{unopened}"
        );
        assert_eq!(
            paired().backend().expect("a whole s3 config"),
            Backend::S3 {
                bucket: "media".into(),
                prefix: object_store::path::Path::default(),
            }
        );
    }
}
