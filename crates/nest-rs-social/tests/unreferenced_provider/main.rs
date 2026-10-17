//! A provider package this binary never names: `nestrs-social-probe` is a
//! dev-dependency of the crate, so rustc receives it with `--extern`, yet no
//! path here reaches it, and its registry entry is not linked. That is why a
//! provider is opened by importing its module, which names the package. Its
//! own binary, since one reference anywhere in a binary links the package for
//! every test in it; the control, which names the package, is
//! `tests/integration/registry.rs`.
//!
//! Linked by mold on Linux (`.cargo/config.toml`) and by the platform's
//! default linker on macOS; the result holds on both.

use nest_rs_social::SocialProviderEntry;

#[test]
fn a_provider_package_no_path_names_is_not_linked() {
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
}
