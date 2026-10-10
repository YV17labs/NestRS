//! Link-time provider registry — the discovery seam, mirroring
//! `nest-rs-health`'s `HealthIndicator`.
//!
//! Each provider `provider.rs` submits one [`SocialProviderEntry`] to a
//! link-time `inventory` registry. [`SocialRegistry`] drains it at the boot's
//! wiring step, before the first lifecycle hook, and asks each entry to build
//! itself ([`resolve_provider`] is the standard implementation), then validates
//! the result — a duplicate key or a key that disagrees with the provider's own
//! [`SocialProvider::key`] **fails boot**.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use nest_rs_config::{Config, ConfigService};
use nest_rs_core::{Container, injectable, inventory};

use crate::provider::SocialProvider;

/// The outcome of building a provider from configuration: `None` means "no
/// credentials set" — the provider stays inert rather than failing boot.
pub type BuiltProvider = Option<Arc<dyn SocialProvider>>;

/// A provider's deployment config, extended with the one question the registry
/// asks before deciding between *inert* and *misconfigured*.
pub trait SocialProviderConfig: Config {
    /// `true` only when the deployment set **none** of this provider's credentials;
    /// a partial config reports `false`, so it fails `validate` and aborts boot.
    fn is_unconfigured(&self) -> bool;
}

/// One provider submitted to the link-time registry by a provider `provider.rs`.
pub struct SocialProviderEntry {
    /// The route/config key this provider is registered under.
    pub key: &'static str,
    /// `type_name::<Provider>()` — used only in the duplicate-key diagnostic.
    pub provider_type_name: fn() -> &'static str,
    /// The provider config's [`Namespaced::NAMESPACE`](nest_rs_config::Namespaced)
    /// — write `GithubSocialConfig::NAMESPACE`, never a hand-typed copy.
    pub config_namespace: &'static str,
    /// Build the provider from the deployment's configuration; [`resolve_provider`]
    /// is the standard implementation.
    pub build: fn(&Container) -> anyhow::Result<BuiltProvider>,
}

/// The standard [`SocialProviderEntry::build`]: a provider in the container wins,
/// then a config in the container, then the provider's own environment. `make`
/// turns a validated config into the concrete provider.
pub fn resolve_provider<P, C>(
    container: &Container,
    make: fn(C) -> anyhow::Result<P>,
) -> anyhow::Result<BuiltProvider>
where
    P: SocialProvider + 'static,
    C: SocialProviderConfig,
{
    if let Some(provider) = container.get::<P>() {
        return Ok(Some(provider as Arc<dyn SocialProvider>));
    }

    let config = match container.get::<C>() {
        // A config in the container is an explicit deployment intent, so even
        // an empty one fails rather than taking the inert path.
        Some(pinned) => (*pinned).clone(),
        None => {
            // `read`, not `C::load()`: it validates after the `is_unconfigured`
            // check, and records the variables claimed so a second reader fails boot.
            let config = nest_rs_config::read::<C>(
                &ConfigService::for_namespace(C::NAMESPACE),
                C::defaults(),
            )?;
            if config.is_unconfigured() {
                return Ok(None);
            }
            config
        }
    };
    config.validate()?;
    Ok(Some(Arc::new(make(config)?)))
}

::nest_rs_core::inventory::collect!(SocialProviderEntry);

/// The resolved set of active social providers, keyed by [`SocialProvider::key`],
/// filled by [`SocialModule`](crate::SocialModule)'s wiring step before the
/// first lifecycle hook runs.
#[injectable]
#[derive(Default)]
pub struct SocialRegistry {
    resolved: OnceLock<HashMap<&'static str, Arc<dyn SocialProvider>>>,
}

impl SocialRegistry {
    /// Drain the registry, build each configured provider and store the map; fails
    /// boot on a partial config, a duplicate key, or a registry/provider key mismatch.
    pub(crate) fn install(&self, container: &Container) -> anyhow::Result<()> {
        let mut resolved: Vec<(&'static str, &'static str, Arc<dyn SocialProvider>)> = Vec::new();

        for entry in inventory::iter::<SocialProviderEntry>() {
            let built = (entry.build)(container).map_err(|err| {
                anyhow::anyhow!(
                    "social provider `{}` is linked but misconfigured: {err}",
                    entry.key,
                )
            })?;
            match built {
                Some(provider) => {
                    resolved.push((entry.key, (entry.provider_type_name)(), provider));
                }
                None => tracing::warn!(
                    target: crate::TARGET,
                    provider = entry.key,
                    // Through `var_name`, so the glob follows a custom prefix.
                    env_namespace = nest_rs_config::var_name(entry.config_namespace, "*"),
                    "linked social provider has no credentials configured; inert",
                ),
            }
        }

        let map = build_registry(resolved)?;

        let keys = sorted_keys(&map);
        tracing::info!(
            target: crate::TARGET,
            providers = keys.join(", "),
            count = keys.len(),
            "registered social providers",
        );

        // A second install — a registry two apps share — keeps the first map.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a second install keeps the first registry, as documented above"
        )]
        let _ = self.resolved.set(map);
        Ok(())
    }

    /// The provider registered under `key`, or `None` for an unknown key
    /// (the caller maps that to a 404).
    pub fn get(&self, key: &str) -> Option<Arc<dyn SocialProvider>> {
        self.resolved.get()?.get(key).cloned()
    }

    /// The registered provider keys, sorted. Empty only on a container no boot
    /// wired (built by hand).
    pub fn keys(&self) -> Vec<&'static str> {
        self.resolved.get().map(sorted_keys).unwrap_or_default()
    }
}

/// The map's keys, sorted — the stable order shared by the boot log and
/// [`SocialRegistry::keys`].
fn sorted_keys(map: &HashMap<&'static str, Arc<dyn SocialProvider>>) -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = map.keys().copied().collect();
    keys.sort_unstable();
    keys
}

/// Validate the resolved entries into the final map, failing on a
/// registry-key/provider-key mismatch or a duplicate key.
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
    use nest_rs_authn::AuthError;
    use nest_rs_oauth_client::{OAuthClient, TokenSet};
    use validator::Validate;

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
        fn client(&self) -> &OAuthClient {
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
        assert!(
            msg.contains("duplicate social provider key `github`"),
            "{msg}"
        );
        assert!(
            msg.contains("OtherGithubSocialProvider"),
            "names both types: {msg}"
        );
    }

    #[test]
    fn build_registry_rejects_a_key_mismatch() {
        let Err(err) = build_registry(vec![("github", "MislabeledProvider", stub("gitlab"))])
        else {
            panic!("entry key disagreeing with provider key must fail boot");
        };
        let msg = err.to_string();
        assert!(msg.contains("key mismatch"), "{msg}");
        assert!(
            msg.contains("gitlab"),
            "names the provider's reported key: {msg}"
        );
    }

    /// A stand-in provider config with a namespace no deployment sets, so the
    /// env leg of `resolve_provider` reads nothing in any environment.
    #[derive(Clone, Default, Validate)]
    struct StubConfig {
        #[validate(length(min = 1))]
        client_id: String,
        #[validate(length(min = 1))]
        client_secret: String,
    }

    impl nest_rs_config::Namespaced for StubConfig {
        const NAMESPACE: &'static str = "social__nestrs_stub_provider";
    }

    impl Config for StubConfig {
        fn from_env(env: &ConfigService, base: Self) -> nest_rs_config::Result<Self> {
            Ok(Self {
                client_id: env.get("CLIENT_ID")?.unwrap_or(base.client_id),
                client_secret: env.get("CLIENT_SECRET")?.unwrap_or(base.client_secret),
            })
        }
    }

    impl SocialProviderConfig for StubConfig {
        fn is_unconfigured(&self) -> bool {
            self.client_id.is_empty() && self.client_secret.is_empty()
        }
    }

    struct BuiltStub {
        key: &'static str,
    }

    impl SocialProvider for BuiltStub {
        fn key(&self) -> &'static str {
            self.key
        }
        fn client(&self) -> &OAuthClient {
            unreachable!("these tests never run the OAuth flow")
        }
        fn profile<'a>(&'a self, _tokens: &'a TokenSet) -> ProfileFuture<'a> {
            Box::pin(async { Err::<SocialProfile, _>(AuthError::Failed("stub".into())) })
        }
    }

    fn build_stub(config: StubConfig) -> anyhow::Result<BuiltStub> {
        assert!(!config.client_id.is_empty(), "only a valid config builds");
        Ok(BuiltStub { key: "stub" })
    }

    fn resolve(container: &Container) -> anyhow::Result<BuiltProvider> {
        resolve_provider::<BuiltStub, StubConfig>(container, build_stub)
    }

    #[test]
    fn an_unconfigured_provider_is_inert_rather_than_a_boot_failure() {
        let container = Container::builder().build();
        let built = resolve(&container).expect("an unconfigured provider must not fail boot");
        assert!(built.is_none(), "no credentials ⇒ no provider");
    }

    #[test]
    fn a_partially_configured_provider_fails_boot() {
        let container = Container::builder()
            .provide(StubConfig {
                client_id: "id".into(),
                client_secret: String::new(),
            })
            .build();
        let Err(err) = resolve(&container) else {
            panic!("a half-configured provider must abort boot");
        };
        assert!(err.to_string().contains("client_secret"), "{err}");
    }

    #[test]
    fn a_container_config_builds_the_provider() {
        let container = Container::builder()
            .provide(StubConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
            })
            .build();
        let built = resolve(&container).expect("a complete config builds");
        assert_eq!(
            built.expect("a provider").key(),
            "stub",
            "the config leg constructs the provider"
        );
    }

    #[test]
    fn a_di_registered_provider_wins_over_config() {
        let container = Container::builder()
            .provide(BuiltStub { key: "pinned" })
            .provide(StubConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
            })
            .build();
        let built = resolve(&container).expect("the DI instance resolves");
        assert_eq!(
            built.expect("a provider").key(),
            "pinned",
            "the container's instance wins",
        );
    }
}
