//! `src/registry.rs` — which entries the link-time registry holds: a provider
//! package's, once the binary names the package. The proof that naming is
//! what links it is `tests/unreferenced_provider/`, which does not.

use nest_rs_core::App;
use nest_rs_social::{SocialModule, SocialProviderEntry, SocialRegistry};
use nestrs_social_probe as _;

#[tokio::test]
async fn a_provider_package_the_binary_names_is_linked_and_active() {
    let keys: Vec<&str> = nest_rs_core::inventory::iter::<SocialProviderEntry>()
        .map(|entry| entry.key)
        .collect();
    assert!(
        keys.contains(&"probe"),
        "the named package's entry: {keys:?}"
    );

    let app = App::new::<SocialModule>().expect("the module boots");
    let registry = app
        .container()
        .get::<SocialRegistry>()
        .expect("SocialModule provides the registry");
    assert!(
        registry.keys().contains(&"probe"),
        "the probe's defaults are complete: {:?}",
        registry.keys(),
    );
}
