//! Link-time provider registry — the discovery seam, mirroring
//! `nest-rs-health`'s `HealthIndicator`.
//!
//! Each provider `module.rs` submits one [`SocialProviderEntry`] to a
//! link-time `inventory` registry. [`SocialProviders`] drains it at bootstrap,
//! filters by [`ReachableProviders`] (an entry whose provider module the app
//! did not import stays inert, with a boot `warn`), resolves each provider
//! through the container, and validates it — a duplicate key or a key that
//! disagrees with the provider's own [`SocialProvider::key`] **fails boot**.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use nest_rs_core::{Container, ReachableProviders, injectable, inventory};

use crate::provider::SocialProvider;

/// One provider submitted to the link-time registry by a provider `module.rs`.
pub struct SocialProviderEntry {
    /// The route/config key this provider is registered under.
    pub key: &'static str,
    /// `TypeId::of::<Provider>()` — checked against [`ReachableProviders`] so
    /// an unreachable provider's entry stays inert (module-gated discovery).
    pub provider_type_id: fn() -> TypeId,
    /// `type_name::<Provider>()` — used only in the duplicate-key diagnostic.
    pub provider_type_name: fn() -> &'static str,
    /// Resolve the concrete provider from the container as a trait object.
    pub resolve: fn(&Container) -> Option<Arc<dyn SocialProvider>>,
}

::nest_rs_core::inventory::collect!(SocialProviderEntry);

/// The resolved set of reachable social providers, keyed by
/// [`SocialProvider::key`]. A plain provider (holds no business logic), it is
/// injected wherever a login flow dispatches on a provider key.
///
/// Populated once at `OnApplicationBootstrap` by
/// [`SocialModule`](crate::SocialModule); [`get`](Self::get) is a map lookup
/// thereafter.
#[injectable]
#[derive(Default)]
pub struct SocialProviders {
    resolved: OnceLock<HashMap<&'static str, Arc<dyn SocialProvider>>>,
}

impl SocialProviders {
    /// Drain the registry, filter by reachability, validate, and store the
    /// resolved map. Returns `Err` — which aborts boot — on a duplicate key or
    /// a registry-key/provider-key mismatch (fail-secure: a silently shadowed
    /// login provider is a security surprise).
    pub(crate) fn install(&self, container: &Container) -> anyhow::Result<()> {
        let reachable = container.get::<ReachableProviders>();
        let mut resolved: Vec<(&'static str, &'static str, Arc<dyn SocialProvider>)> = Vec::new();

        for entry in inventory::iter::<SocialProviderEntry>() {
            let type_id = (entry.provider_type_id)();
            if let Some(r) = reachable.as_ref()
                && !r.0.contains(&type_id)
            {
                tracing::warn!(
                    target: "nest_rs::social",
                    provider = entry.key,
                    "linked social provider is unreachable from the app's module tree; inert",
                );
                continue;
            }

            let Some(provider) = (entry.resolve)(container) else {
                tracing::warn!(
                    target: "nest_rs::social",
                    provider = entry.key,
                    "social provider entry is reachable but could not resolve its provider",
                );
                continue;
            };
            resolved.push((entry.key, (entry.provider_type_name)(), provider));
        }

        let map = build_registry(resolved)?;

        let keys = sorted_keys(&map);
        tracing::info!(
            target: "nest_rs::social",
            providers = keys.join(", "),
            count = keys.len(),
            "registered social providers",
        );

        // OnceLock: a second install (re-boot in one process) is a no-op, not
        // a panic — matches `HealthService::install_container`.
        let _ = self.resolved.set(map);
        Ok(())
    }

    /// The provider registered under `key`, or `None` for an unknown key
    /// (the caller maps that to a 404).
    pub fn get(&self, key: &str) -> Option<Arc<dyn SocialProvider>> {
        self.resolved.get()?.get(key).cloned()
    }

    /// The registered provider keys, sorted. Empty before bootstrap.
    pub fn keys(&self) -> Vec<&'static str> {
        self.resolved.get().map(sorted_keys).unwrap_or_default()
    }
}

/// The map's keys, sorted — the stable order shared by the boot log and
/// [`SocialProviders::keys`].
fn sorted_keys(map: &HashMap<&'static str, Arc<dyn SocialProvider>>) -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = map.keys().copied().collect();
    keys.sort_unstable();
    keys
}

/// Validate the resolved, reachable entries into the final map. Pure over its
/// input (no container, no inventory), so the fail-boot rules are unit-tested
/// directly. Fails on a registry-key/provider-key mismatch or a duplicate key.
fn build_registry(
    resolved: Vec<(&'static str, &'static str, Arc<dyn SocialProvider>)>,
) -> anyhow::Result<HashMap<&'static str, Arc<dyn SocialProvider>>> {
    let mut map: HashMap<&'static str, Arc<dyn SocialProvider>> = HashMap::new();
    let mut seen_types: HashMap<&'static str, &'static str> = HashMap::new();

    for (key, type_name, provider) in resolved {
        // The registry key (what routes match) must agree with the provider's
        // self-reported key (what profiles carry).
        if key != provider.key() {
            anyhow::bail!(
                "social provider key mismatch: registry entry `{key}` (type `{type_name}`) resolves a provider reporting key `{}`",
                provider.key(),
            );
        }
        if let Some(previous) = seen_types.insert(key, type_name) {
            anyhow::bail!(
                "duplicate social provider key `{key}` registered by `{previous}` and `{type_name}`",
            );
        }
        map.insert(key, provider);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use nest_rs_authn::{AuthError, OAuth2Client, TokenSet};

    use super::*;
    use crate::provider::{ProfileFuture, SocialProfile};

    /// A provider whose self-reported key is configurable, so a test can force
    /// the registry-key/provider-key mismatch.
    struct StubProvider {
        reported_key: &'static str,
    }

    impl SocialProvider for StubProvider {
        fn key(&self) -> &'static str {
            self.reported_key
        }
        fn client(&self) -> &OAuth2Client {
            unreachable!("build_registry never touches the client")
        }
        fn profile<'a>(&'a self, _tokens: &'a TokenSet) -> ProfileFuture<'a> {
            Box::pin(async { Err::<SocialProfile, _>(AuthError::Failed("stub".into())) })
        }
    }

    fn stub(reported_key: &'static str) -> Arc<dyn SocialProvider> {
        Arc::new(StubProvider { reported_key })
    }

    #[test]
    fn build_registry_maps_each_key_to_its_provider() {
        let map = build_registry(vec![
            ("github", "GithubSocialProvider", stub("github")),
            ("google", "GoogleSocialProvider", stub("google")),
        ])
        .expect("distinct, self-consistent keys build");
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("github") && map.contains_key("google"));
    }

    #[test]
    fn build_registry_rejects_a_duplicate_key() {
        // `Ok` holds a non-`Debug` map, so match rather than `expect_err`.
        let Err(err) = build_registry(vec![
            ("github", "GithubSocialProvider", stub("github")),
            ("github", "OtherGithubSocialProvider", stub("github")),
        ]) else {
            panic!("two providers under one key must fail boot");
        };
        let msg = err.to_string();
        assert!(msg.contains("duplicate social provider key `github`"), "{msg}");
        assert!(msg.contains("OtherGithubSocialProvider"), "names both types: {msg}");
    }

    #[test]
    fn build_registry_rejects_a_key_mismatch() {
        // Registry entry says "github" but the provider reports "gitlab".
        let Err(err) = build_registry(vec![("github", "MislabeledProvider", stub("gitlab"))])
        else {
            panic!("entry key disagreeing with provider key must fail boot");
        };
        let msg = err.to_string();
        assert!(msg.contains("key mismatch"), "{msg}");
        assert!(msg.contains("gitlab"), "names the provider's reported key: {msg}");
    }

    #[test]
    fn install_filters_out_providers_absent_from_reachable_set() {
        // An empty `ReachableProviders` means every linked entry (github,
        // google) is inert — the registry ends up empty, proving the
        // module-gate. (A hand-built container has neither provider registered
        // anyway, so this also covers the resolve-miss path.)
        let container = Container::builder()
            .provide(ReachableProviders(std::collections::HashSet::new()))
            .build();
        let providers = SocialProviders::default();
        providers.install(&container).expect("install succeeds with an empty set");
        assert!(providers.keys().is_empty(), "unreachable providers must be absent");
        assert!(providers.get("github").is_none());
    }
}
