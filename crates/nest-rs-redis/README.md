# nest-rs-redis

> Part of **[NestRS](https://nestrs.dev)** (alpha). **[Documentation](https://nestrs.dev/queue)** · [Queues (Redis)](https://nestrs.dev/queue/) · [Getting started](https://nestrs.dev/getting-started/)

The **Redis-backed integration** of [`nest-rs-queue`](../nest-rs-queue/) — the
first-class storage for nestrs's job queue layer. Built on
[`apalis`](https://docs.rs/apalis) + apalis-redis: durable, distributed
queues with retries.

This crate is named after **the storage the developer sees** (Redis), not
the underlying framework (apalis). If you swap to SQS, NATS, or in-memory,
you write your own `nestrs-<storage>` crate against `nest-rs-queue` — the
`#[processor]` macro and every `#[inject]` shape stay the same. apalis is
an implementation detail this crate hides.

## What it ships

| Type | Role |
|---|---|
| `QueueConnection` | Shared Redis connection + typed `.of::<Job>("name").push(job)` producer handle. |
| `QueueWorker` | The `Transport` that drains the `ProcessMethod` inventory and runs one apalis worker per `#[process]` method. |
| `QueueModule::for_root(...)` | Activation seam for the producer side: seeds the shared `QueueConnection` in the container. |
| `QueueWorkerModule` | Activation seam for the consumer side: attaches the `QueueWorker` transport to the app. A producer-only app omits this. |
| `QueueConfig` | `#[config(namespace = "queue")]` — `NESTRS_QUEUE__URL`. |

## Wiring

A worker app imports both modules; an API (producer-only) imports just
`QueueModule::for_root(...)`:

```rust,ignore
use nest_rs_core::module;
use nest_rs_redis::{QueueModule, QueueWorkerModule};

#[module(imports = [
    QueueModule::for_root(None),
    QueueWorkerModule,
    // your feature's <Feature>QueueModule
])]
pub struct PlatformWorkerModule;
```

The `#[processor]` macro and `ProcessMethod`, `Job`, `Processor`, and
`JobProducer` types come from [`nest-rs-queue`](../nest-rs-queue/) — this
crate plugs into that contract, it does not redefine it.

## Future expansion

Redis grows beyond queues in many apps — cache, pub/sub, distributed
locks. If those land in nestrs, they ship as **Cargo feature flags on
this crate** (`queue` is enabled by default; `cache`, `pubsub`, … would
join), not as sibling crates. Redis is one external dependency; this is
its one nestrs integration home.

## Wire format

Every job pushed onto Redis is wrapped in a versioned JSON envelope so a
rolling deploy (or a pod restart with un-drained queues) fails closed
instead of misinterpreting bytes:

```json
{ "v": 1, "payload": <the user's job> }
```

`v` is `nest_rs_queue::WIRE_FORMAT_VERSION`. The `#[processor]`-generated
handler unwraps the envelope before deserializing `payload` to the user's
job type. An **unknown** version returns `Err(...)` (no panic — apalis
applies the configured retry budget); an **unversioned** raw value (left
in Redis from a deploy that predates the envelope) is decoded directly
with a `warn` on target `nest_rs::queue`, so jobs already in flight at
upgrade time still drain.

Bumping `WIRE_FORMAT_VERSION` is the breaking change for the queue's
payload layout: producers and consumers must agree on a major version.

## Swapping storage

A new backend is three pieces — `QueueBackend`, `JobProducer`,
`JobConsumer` — plus a `Module` that registers them. See
[`crates/nest-rs-queue/README.md`](../nest-rs-queue/README.md) for the
extension contract. The `#[processor]` macro emits a JSON-erased
`JobHandler` so no backend names the user's `J` and the user never names
a backend's storage type.
