//! Link-time provider registry — the discovery seam, mirroring
//! `nest-rs-health`'s `HealthIndicator`.
//!
//! Each provider `provider.rs` submits one [`SocialProviderEntry`] to a
//! link-time `inventory` registry. [`SocialRegistry`] drains it at bootstrap
//! and asks each entry to build itself ([`resolve_provider`] is the standard
//! implementation), then validates the result — a duplicate key or a key that
//! disagrees with the provider's own [`SocialProvider::key`] **fails boot**.
//!
//! # Who owns an entry, and what decides its fate
//!
//! Discovery is module-gated as everywhere else:
//! [`SocialModule`](crate::SocialModule) owns every entry, so no
//! `SocialModule` in the app's imports means no entry is ever considered.
//! There is no per-provider module — a social provider is not a DI provider
//! (it is never `#[inject]`ed by type, only reached through
//! [`SocialRegistry`] as `Arc<dyn SocialProvider>`), so there is nothing for
//! a module of its own to own.
//!
//! Inside that gate the decision is **configuration**, on the ordinary
//! dual-path `#[config]` rule: a provider that cannot be built is not built.
//!
//! | Config for the provider's namespace | Outcome |
//! |---|---|
//! | Complete — provided in DI, or `NESTRS_SOCIAL__<KEY>__*` | active |
//! | Absent entirely | **inert**, one boot `warn` |
//! | Partial, or invalid | **boot fails**, naming the provider |

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
    /// `true` only when the deployment set **none** of this provider's
    /// credentials. A *partially* set config must report `false` so it fails
    /// `validate` and aborts boot — a half-configured login provider is a
    /// deployment mistake, never a silent opt-out.
    fn is_unconfigured(&self) -> bool;
}

/// One provider submitted to the link-time registry by a provider `provider.rs`.
pub struct SocialProviderEntry {
    /// The route/config key this provider is registered under.
    pub key: &'static str,
    /// `type_name::<Provider>()` — used only in the duplicate-key diagnostic.
    pub provider_type_name: fn() -> &'static str,
    /// The provider config's [`Namespaced::NAMESPACE`](nest_rs_config::Namespaced)
    /// — write `GithubSocialConfig::NAMESPACE`, never a hand-typed copy. The
    /// "not configured" boot warning renders the env prefix from it
    /// ([`env_prefix`]), so the namespace is spelled once, in the `#[config]`
    /// attribute, and a rename cannot leave a stale hint behind.
    pub config_namespace: &'static str,
    /// Build the provider from whatever configuration the deployment supplied.
    /// [`resolve_provider`] is the standard implementation — a provider with an
    /// unusual construction story may write its own.
    pub build: fn(&Container) -> anyhow::Result<BuiltProvider>,
}

/// The standard [`SocialProviderEntry::build`]: a provider already in the
/// container wins, then a config in the container (pinned in code or seeded in
/// a test), then the environment.
///
/// `make` turns a validated config into the concrete provider — for the shared
/// OAuth2 flow that is one `OAuth2Client::new` call.
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
            let config = C::from_env(&ConfigService::for_namespace(C::NAMESPACE))?;
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

/// The resolved set of active social providers, keyed by
/// [`SocialProvider::key`]. A plain provider (holds no business logic), it is
/// injected wherever a login flow dispatches on a provider key.
///
/// Populated once at `OnApplicationBootstrap` by
/// [`SocialModule`](crate::SocialModule); [`get`](Self::get) is a map lookup
/// thereafter.
#[injectable]
#[derive(Default)]
pub struct SocialRegistry {
    resolved: OnceLock<HashMap<&'static str, Arc<dyn SocialProvider>>>,
}

impl SocialRegistry {
    /// Drain the registry, build each configured provider, validate, and store
    /// the resolved map. Returns `Err` — which aborts boot — on a provider that
    /// is partially configured, on a duplicate key, or on a
    /// registry-key/provider-key mismatch (fail-secure: a silently shadowed
    /// login provider is a security surprise).
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
                    target: "nest_rs::social",
                    provider = entry.key,
                    env_namespace = env_prefix(entry.config_namespace),
                    "linked social provider has no credentials configured; inert",
                ),
            }
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

/// The `NESTRS_*` env prefix a config namespace reads from, for the inert
/// provider's boot warning — `social__github` ⇒ `NESTRS_SOCIAL__GITHUB__*`.
/// Mirrors how `ConfigService::for_namespace` builds the same prefix, so the
/// hint always names the variables that would actually be read.
fn env_prefix(namespace: &str) -> String {
    format!("NESTRS_{}__*", namespace.to_uppercase())
}

/// The map's keys, sorted — the stable order shared by the boot log and
/// [`SocialRegistry::keys`].
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
        // Registry entry says "github" but the provider reports "gitlab".
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

    // --- `resolve_provider`: the DI → config → env resolution order ---------

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
        fn from_env(env: &ConfigService) -> nest_rs_config::Result<Self> {
            Ok(Self {
                client_id: env.get("CLIENT_ID").unwrap_or_default(),
                client_secret: env.get("CLIENT_SECRET").unwrap_or_default(),
            })
        }
    }

    impl SocialProviderConfig for StubConfig {
        fn is_unconfigured(&self) -> bool {
            self.client_id.is_empty() && self.client_secret.is_empty()
        }
    }

    /// A concrete provider type `resolve_provider` can register and resolve.
    struct BuiltStub {
        key: &'static str,
    }

    impl SocialProvider for BuiltStub {
        fn key(&self) -> &'static str {
            self.key
        }
        fn client(&self) -> &OAuth2Client {
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
        // Nothing in the container and nothing in the environment for this
        // namespace: the provider opts out silently-but-loudly (the caller
        // logs the warn), it does NOT abort boot.
        let container = Container::builder().build();
        let built = resolve(&container).expect("an unconfigured provider must not fail boot");
        assert!(built.is_none(), "no credentials ⇒ no provider");
    }

    #[test]
    fn a_partially_configured_provider_fails_boot() {
        // The dangerous middle state: someone set the id but not the secret.
        // Treating that as "inert" would silently drop a login the deployment
        // clearly intended to have.
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
        // The provider instance was supplied to the container directly (a
        // provider with an unusual construction story). `resolve_provider` must
        // return *that* instance rather than constructing a second one from the
        // config — otherwise the supplied provider and the registry's copy
        // could diverge.
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
