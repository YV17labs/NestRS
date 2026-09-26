//! In-process integration suite root for `nest-rs-redis` — no Redis. Every
//! test lives in the module named for the `src/` concern it covers: [`queue`]
//! for the producer binding's composition, [`worker`] for what the consumer
//! transport refuses at boot and the panic backstop it wires.

mod queue;
mod worker;
