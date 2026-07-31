use nest_rs_config::{Config, ConfigError, ConfigService, Environment, config};
use validator::Validate;

/// S3-compatible object storage configuration, read from the
/// framework-namespaced `NESTRS_STORAGE__*` keys.
///
/// The defaults target a local S3-compatible server over plain HTTP in
/// path-style addressing (the common shape for MinIO / RustFS in a dev
/// container). For real AWS S3, leave [`endpoint`](Self::endpoint) empty and
/// set [`force_path_style`](Self::force_path_style) to `false`.
#[config(namespace = "storage")]
#[derive(Clone, Validate)]
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
    /// (`NESTRS_STORAGE__ALLOW_HTTP`), defaulting to `true` only in dev/test and
    /// `false` in staging/production (STORAGE-ST2).
    pub allow_http: bool,
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
        }
    }
}

impl Config for StorageConfig {
    /// The unpinned baseline is profile-dependent, and both differences are
    /// security ones. Outside dev/test the dev sentinel credentials
    /// (`nestrs`/`nestrs`) are dropped so an unset `NESTRS_STORAGE__ACCESS_KEY`
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
        Ok(Self {
            endpoint: resolve_endpoint(
                env.get("ENDPOINT").unwrap_or(d.endpoint),
                allow_http,
                &env.var_name("ENDPOINT"),
                &env.var_name("ALLOW_HTTP"),
            )?,
            region: env.get("REGION").unwrap_or(d.region),
            access_key: resolve_credential(
                env.get("ACCESS_KEY").unwrap_or(d.access_key),
                &env.var_name("ACCESS_KEY"),
            )?,
            secret_key: resolve_credential(
                env.get("SECRET_KEY").unwrap_or(d.secret_key),
                &env.var_name("SECRET_KEY"),
            )?,
            bucket: env.get("BUCKET").unwrap_or(d.bucket),
            force_path_style: env.flag("FORCE_PATH_STYLE", d.force_path_style)?,
            allow_http,
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
/// starting healthy and 500-ing on first use. Pure, so the branch is testable
/// without env mutation.
fn resolve_endpoint(
    endpoint: String,
    allow_http: bool,
    endpoint_var: &str,
    allow_http_var: &str,
) -> nest_rs_config::Result<String> {
    if is_plaintext(&endpoint) && !allow_http {
        return Err(ConfigError::parse(
            endpoint_var,
            format!(
                "plain-http endpoint `{endpoint}` is refused because {allow_http_var} is false \
                 (the staging/production default) — credentials and presigned URLs would travel \
                 unencrypted; use an https:// endpoint, or set {allow_http_var}=true to opt in"
            ),
        ));
    }
    Ok(endpoint)
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

/// The resolved credential, refusing a blank one by naming its variable. Blank
/// is only reachable outside dev/test, where [`Config::defaults`] drops the dev
/// sentinel — so this is where STORAGE-ST1 lands as a boot error. Pure, so the
/// branch is testable without env mutation.
fn resolve_credential(resolved: String, var: &str) -> nest_rs_config::Result<String> {
    if resolved.trim().is_empty() {
        return Err(ConfigError::parse(
            var,
            "must be set in staging/production (no dev-credential fallback outside dev/test)",
        ));
    }
    Ok(resolved)
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

    #[test]
    fn credential_blank_aborts_naming_its_variable() {
        // STORAGE-ST1: outside dev/test `Config::defaults` drops the sentinel, so
        // an unset variable arrives here blank and must abort by name.
        let err = resolve_credential(String::new(), "NESTRS_STORAGE__SECRET_KEY")
            .expect_err("must abort");
        assert!(
            err.to_string().contains("NESTRS_STORAGE__SECRET_KEY"),
            "the error names the variable: {err}",
        );
        assert!(
            resolve_credential("   ".into(), "K").is_err(),
            "whitespace-only is blank too",
        );
    }

    #[test]
    fn credential_set_is_taken_verbatim() {
        assert_eq!(
            resolve_credential("AKIAREAL".into(), "K").expect("set ⇒ ok"),
            "AKIAREAL",
        );
    }

    // G1/G2: `with_allow_http` only gates the client's own byte transfers, and
    // only as an opaque request-time 500 — presigning is local, so it minted
    // working plaintext URLs in production. The pairing has to die at load.
    #[test]
    fn a_plain_http_endpoint_is_refused_when_allow_http_is_false() {
        let err = resolve_endpoint(
            "http://minio.internal:9000".into(),
            false,
            "NESTRS_STORAGE__ENDPOINT",
            "NESTRS_STORAGE__ALLOW_HTTP",
        )
        .expect_err("plaintext + allow_http=false must abort boot");
        let rendered = err.to_string();
        assert!(
            rendered.contains("NESTRS_STORAGE__ENDPOINT"),
            "the error names the offending variable: {rendered}",
        );
        assert!(
            rendered.contains("NESTRS_STORAGE__ALLOW_HTTP"),
            "and the opt-in that would allow it: {rendered}",
        );
        // Case-insensitive and whitespace-tolerant — a scheme is not a shibboleth.
        assert!(
            resolve_endpoint("  HTTP://x:9000".into(), false, "E", "A").is_err(),
            "the scheme check must not be defeated by case or leading space",
        );
    }

    #[test]
    fn https_and_the_empty_aws_endpoint_are_always_accepted() {
        for endpoint in ["https://s3.example", "", "https://minio:9000"] {
            assert_eq!(
                resolve_endpoint(endpoint.into(), false, "E", "A").expect("encrypted ⇒ ok"),
                endpoint,
            );
        }
    }

    #[test]
    fn a_plain_http_endpoint_is_accepted_when_allow_http_is_opted_in() {
        // The dev/test default, and the documented production opt-in.
        assert_eq!(
            resolve_endpoint("http://rustfs:9000".into(), true, "E", "A").expect("opted in ⇒ ok"),
            "http://rustfs:9000",
        );
    }

    #[test]
    fn from_env_refuses_the_production_pairing_end_to_end() {
        let err = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [
                    ("NESTRS_STORAGE__ENDPOINT", "http://minio:9000"),
                    ("NESTRS_STORAGE__ALLOW_HTTP", "false"),
                ],
            ),
            StorageConfig::default(),
        )
        .expect_err("the resolved config must not carry a plaintext endpoint");
        assert!(err.to_string().contains("NESTRS_STORAGE__ENDPOINT"));
    }

    // The whole point of the overlay: a pinned bucket must not freeze the
    // credentials or the endpoint alongside it.
    #[test]
    fn env_overrides_each_field_of_a_pinned_config_independently() {
        let pinned = StorageConfig {
            bucket: "pinned-bucket".into(),
            ..Default::default()
        };
        let cfg = StorageConfig::from_env(
            &ConfigService::with_vars(
                "storage",
                [
                    ("NESTRS_STORAGE__ENDPOINT", "https://s3.example"),
                    ("NESTRS_STORAGE__ACCESS_KEY", "AKIAREAL"),
                ],
            ),
            pinned,
        )
        .expect("overlay resolves");
        assert_eq!(cfg.endpoint, "https://s3.example");
        assert_eq!(cfg.access_key, "AKIAREAL");
        assert_eq!(
            cfg.bucket, "pinned-bucket",
            "the pin survives where the env is silent"
        );
        assert_eq!(cfg.secret_key, "nestrs", "and so does the rest of the pin");
    }
}
