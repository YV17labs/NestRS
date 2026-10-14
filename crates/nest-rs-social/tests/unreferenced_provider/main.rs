//! A provider package this binary never names: `nestrs-social-probe` is a
//! dev-dependency of the crate, so rustc receives it with `--extern`, yet no
//! path here reaches it. Its registry entry is not linked, so an app that
//! imports `SocialModule` never holds it. Its own binary, since one reference
//! anywhere in a binary links the package for every test in it; the control,
//! which names the package, is `tests/integration/registry.rs`.
//!
//! Linked by mold on Linux (`.cargo/config.toml`) and by the platform's
//! default linker on macOS; the result holds on both.
#![allow(
    clippy::expect_used,
    reason = "a suite fails by panicking; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

use nest_rs_core::App;
use nest_rs_social::{SocialModule, SocialProviderEntry, SocialRegistry};

#[tokio::test]
async fn a_provider_package_no_path_names_is_not_linked() {
    let keys: Vec<&str> = nest_rs_core::inventory::iter::<SocialProviderEntry>()
        .map(|entry| entry.key)
        .collect();
    assert!(
        keys.contains(&"github"),
        "the crate's own entries are linked: {keys:?}"
    );
    assert!(
        !keys.contains(&"probe"),
        "a dependency line alone links the package's entry: {keys:?}"
    );

    let app = App::new::<SocialModule>().expect("the module boots");
    let registry = app
        .container()
        .get::<SocialRegistry>()
        .expect("SocialModule provides the registry");
    assert!(
        !registry.keys().contains(&"probe"),
        "the probe is complete by default, so only its absence keeps it out: {:?}",
        registry.keys(),
    );
}
