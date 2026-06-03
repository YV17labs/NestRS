//! Typed in-process event bus with decorator-registered handlers.
//!
//! An event is any `Clone + Send + 'static`. A handler is a `#[on_event]`
//! struct that implements [`EventHandler`]; listing it in
//! `#[module(providers = [...])]` (with `EventModule` imported) wires it from
//! the fully-assembled container.
//!
//! Dispatch is in-process and awaited: every handler registered for the event
//! type runs in registration order, each with its own clone.

mod bus;
mod handler;
mod meta;
mod module;

pub use bus::EventBus;
pub use handler::EventHandler;
pub use meta::EventHandlerMeta;
pub use module::EventModule;

pub use nestrs_events_macros::on_event;

pub use async_trait::async_trait;
