# nestrs-events

Typed in-process event bus for nestrs — the `@nestjs/event-emitter` analog.

Emit a `Clone + Send + 'static` event with `EventBus::emit(event)`; every
`#[on_event]`-tagged method discovered under a `#[listeners]` impl runs in
registration order. Dispatch is in-process, awaited, and routed by `TypeId`.

## Extending

This crate carries a *single* mechanism: typed, in-process, `TypeId`-keyed
dispatch. That mechanism is the seam. To swap it for a different transport
(NATS, Redis Streams, Kafka) you do not subclass `EventBus` — you ship a
sibling crate with its own dispatcher and its own decorator (e.g. a
`#[subject = "orders.placed"]` attribute), because over-the-wire delivery
demands a serialized envelope and a routing key that `TypeId` cannot supply.

If you need an alternative *in-process* dispatch policy (concurrent fan-out,
priority ordering, retries on a panicking listener), open an issue — that
fits behind the current API and is a single-bus extension, not a sibling
crate.

A community impl crossing process boundaries would be named e.g.
`nestrs-events-nats`. It exposes its own bus type and its own
`#[subscribe(subject = "…")]` decorator; apps that need both transports
inject both buses.
