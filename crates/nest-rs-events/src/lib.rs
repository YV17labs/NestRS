//! Typed in-process event bus with decorator-registered listeners.
//!
//! An event is any `Clone + Send + 'static`. Listeners live as methods on a
//! regular `#[injectable]` provider, grouped under `#[listeners]` on the
//! `impl` block, each tagged `#[on_event]`. Listing the provider in
//! `#[module(providers = [...])]` (with `EventsModule` imported) wires every
//! listener from the fully-assembled container at bootstrap.
//!
//! Dispatch is in-process and awaited: every listener registered for the
//! event type runs in registration order, each with its own clone.

#![warn(missing_docs)]

/// This crate's span target — The in-process event bus and its listeners.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::events";

mod bus;
mod inventory;
mod module;

pub use bus::EventBus;
pub use inventory::ListenerMethod;
pub use module::EventsModule;

pub use nest_rs_events_macros::listeners;
