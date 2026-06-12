use nest_rs_config::{Config, ConfigService, config};
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
    #[validate(length(min = 1, message = "must not be empty"))]
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    #[validate(length(min = 1, message = "must not be empty"))]
    pub bucket: String,
    /// `true` ⇒ path-style addressing (`endpoint/bucket/key`), required by
    /// most S3-compatible servers. `false` ⇒ virtual-hosted-style
    /// (`bucket.endpoint/key`), the AWS default.
    pub force_path_style: bool,
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
        }
    }
}

impl Config for StorageConfig {
    fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
        let d = Self::default();
        Ok(Self {
            endpoint: env.get("ENDPOINT").unwrap_or(d.endpoint),
            region: env.get("REGION").unwrap_or(d.region),
            access_key: env.get("ACCESS_KEY").unwrap_or(d.access_key),
            secret_key: env.get("SECRET_KEY").unwrap_or(d.secret_key),
            bucket: env.get("BUCKET").unwrap_or(d.bucket),
            force_path_style: env.flag("FORCE_PATH_STYLE", d.force_path_style)?,
        })
    }
}
