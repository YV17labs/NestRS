//! Integration suite root for `nest-rs-events`. Every test lives in the
//! module named for the `src/` concern it covers: [`bus`] for emission
//! through the discovered `#[on_event]` methods, [`order`] for the
//! deterministic dispatch-order guarantee.

mod bus;
mod order;
