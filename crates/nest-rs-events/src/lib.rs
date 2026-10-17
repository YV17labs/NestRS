//! Typed in-process event bus with decorator-registered listeners.
//!
//! An event is any `Clone + Send + 'static`. Listeners live as methods on a
//! regular `#[injectable]` provider, grouped under `#[listeners]` on the
//! `impl` block, each tagged `#[on_event]`. Listing the provider in
//! `#[module(providers = [...])]` (with `EventsModule` imported) subscribes
//! every listener from the fully-assembled container as the boot ends, before
//! the first lifecycle hook, so an event an init hook emits reaches them.
//!
//! Dispatch is in-process and awaited: every listener registered for the
//! event type runs in registration order, each with its own clone — once the
//! emitter's transaction has committed when it emits inside one, and never when
//! that transaction rolls back (see [`EventBus::emit`]).

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the in-process event bus and its listeners.
pub const TARGET: &str = "nest_rs::events";

mod bus;
mod inventory;
mod module;
pub mod unit;

pub use bus::EventBus;
pub use inventory::ListenerMethod;
pub use module::{EventsModule, NO_BUS_REPORT};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::bus::subscribe_named;
}

/// Orchestrator on a provider's `impl` block: each `#[on_event]` method in it
/// listens on the [`EventBus`] for the event type it takes.
///
/// ```
/// # use std::collections::HashMap;
/// # use anyhow::Context as _;
/// # use parking_lot::Mutex;
/// # use nest_rs_core::{App, injectable, module};
/// # use nest_rs_events::{EventBus, EventsModule, ListenerMethod, listeners};
/// # #[derive(Clone)]
/// # pub struct PointsAwarded { user_id: u64, amount: i64 }
/// # #[derive(Clone)]
/// # pub struct PointsRedeemed { user_id: u64, amount: i64 }
/// # #[injectable]
/// # #[derive(Default)]
/// # pub struct Ledger { balances: Mutex<HashMap<u64, i64>> }
/// # impl Ledger {
/// #     async fn credit(&self, user_id: u64, amount: i64) { *self.balances.lock().entry(user_id).or_default() += amount; }
/// #     async fn debit(&self, user_id: u64, amount: i64) { *self.balances.lock().entry(user_id).or_default() -= amount; }
/// #     fn balance(&self, user_id: u64) -> i64 { self.balances.lock().get(&user_id).copied().unwrap_or_default() }
/// # }
/// #[injectable]
/// pub struct PointsHandlers {
///     #[inject] svc: std::sync::Arc<Ledger>,
/// }
///
/// #[listeners]
/// impl PointsHandlers {
///     #[on_event]
///     async fn on_awarded(&self, e: PointsAwarded) {
///         self.svc.credit(e.user_id, e.amount).await;
///     }
///
///     #[on_event]
///     async fn on_redeemed(&self, e: PointsRedeemed) {
///         self.svc.debit(e.user_id, e.amount).await;
///     }
/// }
/// # #[module(imports = [EventsModule], providers = [Ledger, PointsHandlers])]
/// # struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let app = App::new::<AppModule>()?;
/// # let bus = app.container().get::<EventBus>().context("EventsModule provides the bus")?;
/// # let ledger = app.container().get::<Ledger>().context("Ledger is provided")?;
///
/// bus.emit(PointsAwarded { user_id: 1, amount: 10 }).await;
/// bus.emit(PointsRedeemed { user_id: 1, amount: 3 }).await;
/// assert_eq!(ledger.balance(1), 7);
///
/// let wired: Vec<_> = nest_rs_core::inventory::iter::<ListenerMethod>().map(|l| l.name).collect();
/// assert!(wired.contains(&"PointsHandlers::on_awarded"));
/// # Ok(())
/// # }
/// ```
pub use nest_rs_events_macros::listeners;
