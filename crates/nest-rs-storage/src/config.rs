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
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        let environment = Environment::from_env();
        let dev = !matches!(environment, Environment::Production | Environment::Staging);
        let d = Self::default();
        Ok(Self {
            endpoint: env.get("ENDPOINT").unwrap_or(d.endpoint),
            region: env.get("REGION").unwrap_or(d.region),
            // Credentials must never silently fall back to the dev sentinel
            // `nestrs`/`nestrs` in staging/production (STORAGE-ST1).
            access_key: resolve_credential(
                env.get("ACCESS_KEY"),
                "NESTRS_STORAGE__ACCESS_KEY",
                dev,
                d.access_key,
            )?,
            secret_key: resolve_credential(
                env.get("SECRET_KEY"),
                "NESTRS_STORAGE__SECRET_KEY",
                dev,
                d.secret_key,
            )?,
            bucket: env.get("BUCKET").unwrap_or(d.bucket),
            force_path_style: env.flag("FORCE_PATH_STYLE", d.force_path_style)?,
            // Plain-HTTP is a dev convenience; default it off outside dev/test so
            // production credentials never travel unencrypted by omission.
            allow_http: env.flag("ALLOW_HTTP", dev)?,
        })
    }
}

/// A storage credential from the environment, defaulting to the dev sentinel
/// **only** in dev/test; unset (or blank) in staging/production aborts boot.
/// Pure, so the profile-dependent branch is testable without env mutation.
fn resolve_credential(
    raw: Option<String>,
    var: &str,
    dev: bool,
    dev_default: String,
) -> nest_rs_config::Result<String> {
    match raw {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ if dev => Ok(dev_default),
        _ => Err(ConfigError::parse(
            var,
            "must be set in staging/production (no dev-credential fallback outside dev/test)",
        )),
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
    fn credential_defaults_to_the_dev_sentinel_only_in_dev() {
        assert_eq!(
            resolve_credential(None, "NESTRS_STORAGE__ACCESS_KEY", true, "nestrs".into())
                .expect("dev falls back"),
            "nestrs",
        );
        assert_eq!(
            resolve_credential(Some("  ".into()), "K", true, "nestrs".into())
                .expect("blank ⇒ default in dev"),
            "nestrs",
        );
    }

    #[test]
    fn credential_unset_aborts_outside_dev() {
        // STORAGE-ST1: no silent `nestrs`/`nestrs` in staging/production.
        let err = resolve_credential(None, "NESTRS_STORAGE__SECRET_KEY", false, "nestrs".into())
            .expect_err("must abort");
        assert!(
            err.to_string().contains("NESTRS_STORAGE__SECRET_KEY"),
            "the error names the variable: {err}",
        );
        assert!(
            resolve_credential(Some(String::new()), "K", false, "nestrs".into()).is_err(),
            "blank also aborts outside dev",
        );
    }

    #[test]
    fn credential_set_is_taken_verbatim_in_every_profile() {
        for dev in [true, false] {
            assert_eq!(
                resolve_credential(Some("AKIAREAL".into()), "K", dev, "nestrs".into())
                    .expect("set ⇒ ok"),
                "AKIAREAL",
            );
        }
    }
}
