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

/// A linked provider with no credentials is **inert**, and inert is silent to
/// every caller: `SocialRegistry::get` simply answers `None`, exactly as it
/// would for a provider nobody wrote.
///
/// So the boot warn is the only thing standing between "we deliberately did not
/// configure Google" and "the Google button has been dead since the deploy that
/// dropped the secret". It carries the env glob rather than a variable name so
/// the remedy follows a custom `NESTRS_ENV_PREFIX` instead of naming variables
/// the app would never read.
#[tokio::test]
async fn a_linked_provider_with_no_credentials_is_reported_inert_with_its_env_namespace() {
    let logs = nest_rs_testing::LogCapture::install();
    // No `NESTRS_SOCIAL_*` credentials in the test environment, so both
    // first-party providers build to `None`.
    let app = nest_rs_core::App::new::<nest_rs_social::SocialModule>().expect("the module boots");
    app.init().await.expect("the bootstrap phase drains");

    let inert = logs.find(
        "nest_rs::social",
        "linked social provider has no credentials configured; inert",
    );
    assert_eq!(
        inert.len(),
        2,
        "both first-party providers report, one line each: {:#?}",
        logs.events(),
    );
    for event in &inert {
        assert_eq!(event.level, "warn");
        let namespace = event
            .field("env_namespace")
            .expect("the event carries the env glob");
        assert!(
            namespace.contains('*'),
            "the remedy is a glob, not a variable list: {namespace}",
        );
        assert!(
            event.field("provider").is_some(),
            "the event names which provider went inert, got {:?}",
            event.fields,
        );
    }
}
