//! `OAuthServerConfig` (`src/config.rs`) from outside the crate.

use nest_rs_oauth_server::OAuthServerConfig;

/// The overlay is written by hand, so a product reads the field's binding as it
/// reads one `#[config]` derived.
#[test]
fn a_product_reads_the_registry_through_its_binding() {
    let field = OAuthServerConfig::<()>::CLIENTS;
    assert_eq!(
        (field.key(), field.path()),
        ("CLIENTS", "OAuthServerConfig::clients")
    );
}
