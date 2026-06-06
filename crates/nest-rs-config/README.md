# nest-rs-config

Typed, namespaced configuration for nestrs — the `@nestjs/config` analog.

A `#[config(namespace = "…")]` struct maps `NESTRS_<NAMESPACE>__<KEY>`
variables to fields via an explicit `from_env`. `ConfigModule::for_root()`
merges the `.env` cascade once; `ConfigModule::for_feature::<T>()` loads
and registers `Arc<T>` for injection.

## Extending

The value source is pluggable via [`ConfigSource`]:

- `trait ConfigSource: Send + Sync + 'static { fn get(&self, var: &str) -> Option<String>; }`
- [`EnvSource`] is the default — process env + `.env` cascade.
- Build a custom reader with `ConfigService::with_source(namespace, Arc::new(MySource))`.

A `from_env` reads through `ConfigService`, so swapping the source swaps
*every* config's input transparently. The trait is **sync** by design: a
remote source (Vault, K8s ConfigMap, AWS Parameter Store) pre-fetches its
keys into an in-memory map at startup and serves `get` from that map.

A community impl is named `nest-rs-config-<backend>` — e.g.
`nest-rs-config-vault`, `nest-rs-config-k8s-configmap`. It exposes a
`ConfigSource` implementor plus an optional helper that constructs a
populated `ConfigService` for a given namespace.

The `NESTRS_<NS>__<KEY>` scheme is the framework contract — alternative
sources should accept these names as their keys (or document a stable
mapping such as `NESTRS_DB__URL` → `secret/data/nestrs/db#url`).
