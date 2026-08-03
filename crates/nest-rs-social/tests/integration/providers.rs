//! The first-party providers (`src/providers/`): each submits its link-time
//! registry entry through the same public seam a third-party crate uses.

use nest_rs_social::SocialProviderEntry;

#[test]
fn both_first_party_providers_submit_a_registry_entry() {
    let keys: Vec<&str> = nest_rs_core::inventory::iter::<SocialProviderEntry>()
        .map(|entry| entry.key)
        .collect();
    assert!(
        keys.contains(&"github"),
        "github entry must be linked: {keys:?}"
    );
    assert!(
        keys.contains(&"google"),
        "google entry must be linked: {keys:?}"
    );
}
