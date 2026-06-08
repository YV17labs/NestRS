# nest-rs-queue

> Part of **[NestRS](https://nestrs.dev)** (alpha). **[Documentation](https://nestrs.dev/queue)** · [Queues](https://nestrs.dev/queue/) · [Getting started](https://nestrs.dev/getting-started/)

The **backend-agnostic** contract that opens nestrs's queue layer to multiple
storage technologies. Today's first-class backend is **Redis** (via
apalis-redis), shipped as [`nest-rs-redis`](../nest-rs-redis/). A third-party
`nestrs-<storage>` — SQS, NATS JetStream, an in-memory test backend, a SQL
queue — depends on this crate and implements the seams below. No change to
the `#[processor]` macro, no change to application code.

## The seams

A backend plugs in by implementing three traits and shipping a `Module` that
registers them. Everything else (the `#[processor]` macro, the
`ProcessMethod` link-time registry, the `Job` marker) is shared.

| Trait | What it does |
|---|---|
| [`QueueBackend`] | Identifies the backend by name. Surfaced in boot diagnostics. |
| [`JobProducer`] | `push_json(queue, payload)` — enqueue a JSON-encoded job. The typed `push::<J>` lives as an extension method on [`JobProducerExt`]. |
| [`JobConsumer`] | `run(methods, container, cancel)` — drain the access-graph-filtered list of `#[process]` methods until shutdown. |

The crate also re-exports `async_trait` so backends and macros don't take a
direct dependency, and ships the `#[processor]` macro (forwarded from
`nest-rs-queue-macros`) so the call site keeps writing `use
nest_rs_queue::processor;` regardless of which storage integration is wired
in.

## The link-time registry

The `#[processor]` macro (in `nest-rs-queue-macros`, re-exported by this
crate) submits one [`ProcessMethod`] per `#[process]`-tagged method to a
global `inventory` registry. A backend's `JobConsumer::run` drains that
registry — already filtered by
[`ReachableProviders`](::nest_rs_core::ReachableProviders) so methods on
unreachable providers are silently skipped — and dispatches through each
entry's [`JobHandler`].

`JobHandler` is **type-erased** on purpose: it takes a `serde_json::Value`
payload and the assembled `Container`, then deserializes to the user's `J`
inside the closure the macro emits. The wire format is always JSON; a
backend may re-encode internally, but the macro never names a backend's
types and a backend never names a user's `J`.

```rust,ignore
pub type JobHandler = fn(
    payload: serde_json::Value,
    container: nest_rs_core::Container,
) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send>>;
```

## How to add a new backend

1. **Depend on `nest-rs-queue`** (this crate) — get the abstractions and the
   `#[processor]` macro through the same import root. Application code in
   your downstream users keeps writing `nest_rs_queue::processor`
   unchanged.
2. **Implement [`QueueBackend`], [`JobProducer`], [`JobConsumer`]** for
   your concrete types (a connection, a producer handle, a consumer
   driver).
3. **Ship a `Module`** that:
   - seeds your `JobProducer` in the container (typically via
     `provide_factory` so the connection opens asynchronously at boot),
     and
   - contributes a `Transport` whose `serve` constructs your
     `JobConsumer`, drains the `ProcessMethod` inventory filtered by
     `ReachableProviders`, and calls `JobConsumer::run` with the
     cancellation token.
4. **The `#[processor]` macro and the `ProcessMethod` registration work
   unchanged.** Application code keeps writing `#[processor]` + `#[process]`
   exactly as it does today; switching storage is a Cargo dependency +
   module-import change.

The Redis implementation in [`crates/nest-rs-redis/`](../nest-rs-redis/) is
the reference — `QueueConnection` (producer) wraps
`RedisStorage<serde_json::Value>`, `QueueWorker` (consumer) is the
`Transport`, `QueueModule` / `QueueWorkerModule` are the activation seams.
A new backend mirrors the same three-piece shape.

## Why JSON-erased and not generic-over-J?

A typed `JobConsumer<J>` would force every backend to monomorphize one
worker per `(queue, J)` pair and would couple the macro to the backend's
storage type (e.g. apalis-redis's `RedisStorage<J>`). Type-erasing at the
inventory boundary keeps the macro free of backend names and lets each
backend pick its own internal storage strategy. The cost is one
`serde_json::from_value` per job — negligible against any network round
trip.
