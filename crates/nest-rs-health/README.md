# nest-rs-health

> Part of **[NestRS](https://nestrs.dev)** (alpha). **[Documentation](https://nestrs.dev/health)** · [Health probes](https://nestrs.dev/health/) · [Getting started](https://nestrs.dev/getting-started/)

Liveness/readiness/startup probes for nestrs. Importing `HealthModule`
mounts three routes (`GET /health/live`, `GET /health/ready`,
`GET /health/startup`); each runs every registered indicator for its
`ProbeKind` and returns `200` (all up) or `503` (any down) with a JSON body.

## Extending

The extension surface is the indicator itself — there is no backend to
swap. An app (or another `crates/nestrs-*` crate) tags methods on an
`#[injectable]` provider's impl block:

```rust
#[indicators]
impl DbProbes {
    #[readiness]
    async fn database(&self) -> anyhow::Result<()> {
        self.pool.ping().await
    }

    #[liveness]
    async fn process_alive(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
```

Each method submits one `HealthIndicator` to a link-time `inventory`
registry; `HealthService` drains the registry at probe time and filters by
`ReachableProviders` (an unreachable provider's indicators do not run).

The wire format (Terminus-shaped JSON: `status`, `info`, `error`,
`details`) is fixed — operators read it across deployments. Adding a probe
class beyond `liveness`/`readiness`/`startup` is a framework change, not an
app extension; the three names map to the Kubernetes probe types directly,
so the closed set is intentional.

A community crate shipping an indicator pack (Redis, S3, Kafka, …) is
named e.g. `nest-rs-health-<backend>`. It exposes an `#[injectable]`
provider with `#[indicators]` methods plus a `<Backend>HealthModule` that
registers the provider — listing the module activates the checks.
