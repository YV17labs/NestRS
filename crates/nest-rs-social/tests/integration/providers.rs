//! The first-party providers (`src/providers/`): each submits its link-time
//! registry entry through the same public seam a third-party crate uses, and
//! is opened by its own module.

use nest_rs_core::{App, Module, module};
use nest_rs_social::{GithubSocialModule, GoogleSocialModule, SocialModule};

use crate::{linked_keys, registry_keys, seeded_github_config, seeded_google_config};

#[module(imports = [SocialModule, GithubSocialModule, GoogleSocialModule])]
struct BothProvidersModule;

#[module(imports = [SocialModule, GithubSocialModule])]
struct GithubLoginModule;

#[module(imports = [SocialModule, GoogleSocialModule])]
struct GoogleLoginModule;

/// Every first-party provider gets credentials, so a provider left out of the
/// registry was closed by its module, never inert.
async fn keys_with_every_provider_seeded<M: Module>() -> Vec<&'static str> {
    registry_keys(
        App::builder()
            .module::<M>()
            .provide(seeded_github_config())
            .provide(seeded_google_config()),
    )
    .await
}

#[test]
fn both_first_party_providers_submit_a_registry_entry() {
    let keys = linked_keys();
    assert!(
        keys.contains(&"github"),
        "github entry must be linked: {keys:?}"
    );
    assert!(
        keys.contains(&"google"),
        "google entry must be linked: {keys:?}"
    );
}

/// An imported provider with no credentials is inert and silent to every caller:
/// the boot warn, carrying the env glob, is its only trace.
#[tokio::test]
async fn an_imported_provider_with_no_credentials_is_reported_inert_with_its_env_namespace() {
    let logs = nest_rs_testing::LogCapture::install();
    // No `<PREFIX>_SOCIAL__*` credentials in the test environment, so both
    // first-party providers build to `None`.
    let _app = App::new::<BothProvidersModule>().expect("the module boots");

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

/// A first-party provider is opened by its module: an app importing
/// `SocialModule` alone holds no provider, even with every credential set.
#[tokio::test]
async fn a_provider_whose_module_the_app_does_not_import_stays_closed() {
    let logs = nest_rs_testing::LogCapture::install();
    let keys = keys_with_every_provider_seeded::<SocialModule>().await;
    assert!(
        keys.is_empty(),
        "credentials alone open no provider: {keys:?}"
    );

    let skipped = logs.find(
        "nest_rs::social",
        "linked social provider's module is not imported; skipped",
    );
    for provider in ["github", "google"] {
        assert!(
            skipped.iter().any(|event| event.level == "debug"
                && event.field("provider").as_deref() == Some(provider)),
            "a closed provider is said at debug, naming {provider}: {:#?}",
            logs.events(),
        );
    }
}

#[tokio::test]
async fn the_github_module_opens_github_alone() {
    let keys = keys_with_every_provider_seeded::<GithubLoginModule>().await;
    assert_eq!(keys, ["github"], "its module opens it, and nothing else");
}

#[tokio::test]
async fn the_google_module_opens_google_alone() {
    let keys = keys_with_every_provider_seeded::<GoogleLoginModule>().await;
    assert_eq!(keys, ["google"], "its module opens it, and nothing else");
}
