# nestrs-throttler

Rate limiting for nestrs — a per-route `ThrottlerGuard` reading a
`#[meta(Throttle::...)]` override, backed by an in-memory fixed-window
counter.

`ThrottlerModule::for_root(None)` loads `NESTRS_THROTTLER__*` and registers
the shared [`InMemoryThrottler`]. Bind the guard per route with
`#[use_guards(ThrottlerGuard)]`; over-limit requests get `429 Too Many
Requests` with `Retry-After`.

## Extending

The open seam is the [`ThrottlerStore`] trait — the counting policy a
guard interrogates:

```rust
pub trait ThrottlerStore: Send + Sync + 'static {
    fn hit(&self, key: &str, limit: Throttle) -> Decision;
    fn default_limit(&self) -> Throttle;
    fn trusted_proxies(&self) -> &[IpAddr];
}
```

`InMemoryThrottler` is the default impl. The trait is **sync** by design
— a Redis-backed implementor wraps a non-async driver (`redis::Commands`)
on a dedicated thread, or uses `tokio::task::block_in_place`. Keeping the
trait sync avoids an extra await point on the in-memory hot path.

A community impl is named `nestrs-throttler-<backend>` — e.g.
`nestrs-throttler-redis` (sliding window, distributed),
`nestrs-throttler-memcached`. The recommended shape:

1. An `#[injectable]` struct implementing `ThrottlerStore`.
2. A `<Name>Module` that registers the implementor as `Arc<Self>` from a
   factory.
3. A `<Name>ThrottlerGuard` that injects `Arc<<Name>Throttler>` (or
   `Arc<dyn ThrottlerStore>` if generic) and otherwise mirrors
   [`ThrottlerGuard`]'s logic — bind it instead of the in-memory guard on
   routes that need the distributed backend.

The framework `ThrottlerGuard` intentionally injects the concrete
`InMemoryThrottler`: a per-route guard is small (~50 LOC) and an alternative
backend usually wants a different identity-key policy anyway (a Redis store
may want a per-tenant prefix, an L4 store may want a different forwarded-IP
extraction). Duplicate the guard, share the trait.
