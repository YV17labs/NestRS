//! The kit's modules — one per case, each making its case's processor alone
//! reachable, so a case's worker drains its own case's queue: on a store the
//! cases running in parallel share, another case's jobs are another worker's.

use nest_rs_core::{Module, module};

use super::command::{
    BudgetQueue, ConcurrencyQueue, DeathQueue, DelayQueue, DrainQueue, OnceQueue, RenewalQueue,
    RetryQueue, StallQueue, TakenQueue, TraceQueue,
};
use super::processor::{
    BudgetProcessor, ConcurrencyProcessor, DeathProcessor, DelayProcessor, DrainProcessor,
    OnceProcessor, RenewalProcessor, RetryProcessor, StallProcessor, TakenProcessor,
    TraceProcessor,
};

/// The module a case's worker imports beside the backend's.
pub(crate) trait CaseModule {
    /// The module making the case's processor reachable.
    type Module: Module + 'static;
}

macro_rules! modules {
    ($($queue:ident => $module:ident($processor:ident);)+) => {
        $(
            #[doc = concat!("The case on `", stringify!($queue), "`, reachable.")]
            #[module(providers = [$processor])]
            pub(crate) struct $module;

            impl CaseModule for $queue {
                type Module = $module;
            }
        )+
    };
}

modules! {
    OnceQueue => OnceModule(OnceProcessor);
    ConcurrencyQueue => ConcurrencyModule(ConcurrencyProcessor);
    RetryQueue => RetryModule(RetryProcessor);
    BudgetQueue => BudgetModule(BudgetProcessor);
    DeathQueue => DeathModule(DeathProcessor);
    TakenQueue => TakenModule(TakenProcessor);
    StallQueue => StallModule(StallProcessor);
    DrainQueue => DrainModule(DrainProcessor);
    RenewalQueue => RenewalModule(RenewalProcessor);
    DelayQueue => DelayModule(DelayProcessor);
    TraceQueue => TraceModule(TraceProcessor);
}
