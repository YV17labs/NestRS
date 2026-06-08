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

mod bus;
mod inventory;
mod module;

pub use bus::EventBus;
pub use inventory::ListenerMethod;
pub use module::EventsModule;

pub use nest_rs_events_macros::listeners;
