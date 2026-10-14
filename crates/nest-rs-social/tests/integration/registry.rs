//! `src/registry.rs` — which entries the registry keeps: a provider package
//! is linked once the binary names it, and active only in an app importing
//! its module. The proof that naming is what links it is
//! `tests/unreferenced_provider/`, which does not.

use nest_rs_core::{App, Module, module};
use nest_rs_social::SocialModule;
use nestrs_social_probe::ProbeSocialModule;

use crate::{linked_keys, registry_keys};

#[module(imports = [SocialModule, ProbeSocialModule])]
struct ProbeLoginModule;

async fn keys_of_an_app_importing<M: Module>() -> Vec<&'static str> {
    registry_keys(App::builder().module::<M>()).await
}

#[test]
fn a_provider_package_the_binary_names_is_linked() {
    let keys = linked_keys();
    assert!(
        keys.contains(&"probe"),
        "the named package's entry: {keys:?}"
    );
}

#[tokio::test]
async fn a_linked_provider_package_stays_closed_in_an_app_importing_social_module_alone() {
    let keys = keys_of_an_app_importing::<SocialModule>().await;
    assert!(
        !keys.contains(&"probe"),
        "linking opens no provider, though the probe is complete by default: {keys:?}",
    );
}

#[tokio::test]
async fn a_provider_package_whose_module_the_app_imports_is_active() {
    let keys = keys_of_an_app_importing::<ProbeLoginModule>().await;
    assert_eq!(keys, ["probe"], "its module opens it, and nothing else");
}
