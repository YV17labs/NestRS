//! Typed in-process event bus with decorator-registered listeners.
//!
//! An event is any `Clone + Send + 'static`. Listeners live as methods on a
//! regular `#[injectable]` provider, grouped under `#[listeners]` on the
//! `impl` block, each tagged `#[on_event]`. Listing the provider in
//! `#[module(providers = [...])]` (with `EventModule` imported) wires every
//! listener from the fully-assembled container at bootstrap.
//!
//! Dispatch is in-process and awaited: every listener registered for the
//! event type runs in registration order, each with its own clone.

mod bus;
mod meta;
mod module;

pub use bus::EventBus;
pub use meta::ListenerMethod;
pub use module::EventModule;

pub use nest_rs_events_macros::listeners;
