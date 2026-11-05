//! `StorageConfig` (`src/config.rs`) from outside the crate.

use nest_rs_storage::StorageConfig;

/// The overlay is written by hand, so a product reads each field's binding as it
/// reads one `#[config]` derived.
#[test]
fn a_product_reads_every_field_through_its_binding() {
    for (field, key, path) in [
        (StorageConfig::URL, "URL", "StorageConfig::url"),
        (
            StorageConfig::ENDPOINT,
            "ENDPOINT",
            "StorageConfig::endpoint",
        ),
        (StorageConfig::REGION, "REGION", "StorageConfig::region"),
        (
            StorageConfig::FORCE_PATH_STYLE,
            "FORCE_PATH_STYLE",
            "StorageConfig::force_path_style",
        ),
        (
            StorageConfig::ACCESS_KEY,
            "ACCESS_KEY",
            "StorageConfig::access_key",
        ),
        (
            StorageConfig::SECRET_KEY,
            "SECRET_KEY",
            "StorageConfig::secret_key",
        ),
        (
            StorageConfig::ALLOW_HTTP,
            "ALLOW_HTTP",
            "StorageConfig::allow_http",
        ),
        (StorageConfig::TLS, "TLS", "StorageConfig::tls"),
    ] {
        assert_eq!((field.key(), field.path()), (key, path));
    }
    assert_eq!(
        StorageConfig::OPERATION_TIMEOUT.key(),
        "OPERATION_TIMEOUT_SECS"
    );
    assert_eq!(StorageConfig::READ_TIMEOUT.key(), "READ_TIMEOUT_SECS");
}
