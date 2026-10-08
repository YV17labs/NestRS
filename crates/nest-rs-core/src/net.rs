//! [`Net`] — what a port waits on a call before it gives up — and the order the
//! boot holds between nets and the [`Budget`]s they reach.

use std::any::{Any, TypeId};
use std::collections::HashSet;
use std::time::Duration;

use crate::access::{ModuleDescriptor, injection_closure};
use crate::budget::Budget;
use crate::container::ContainerBuilder;
use crate::error::BudgetPastNetError;

/// What a port waits on a call before it gives up and answers in its own
/// terms: a guard denies, a push fails, a claim skips its occurrence.
///
/// A net bounds a backend that stopped bounding itself; a resource's own
/// [`Budget`] must fail first, with its cause, so the boot refuses a budget at
/// or past a net that reaches it ([`BudgetPastNetError`]). Declared as metadata
/// (`builder.provide_meta(Net::over::<R>(…))`).
#[derive(Debug)]
pub struct Net {
    port: &'static str,
    wait: Duration,
    reach: Reach,
}

/// Which budgets a net reaches.
#[derive(Clone, Copy, Debug)]
enum Reach {
    /// The one resource a binding handed the port.
    Over(TypeId),
    /// Whatever a provider's code reaches: what it injects, transitively, and
    /// every ambient resource.
    Around(TypeId),
}

impl Net {
    /// The net `port` waits `wait` under, over the resource `R` a binding hands
    /// it — the queue port's over the Redis connection its binding holds.
    ///
    /// `port` names it in a sentence, before "'s net" (`"the queue port"`).
    pub fn over<R: Any>(port: &'static str, wait: Duration) -> Self {
        Self {
            port,
            wait,
            reach: Reach::Over(TypeId::of::<R>()),
        }
    }

    /// The net `port` waits `wait` under around the provider `P`'s code — a
    /// guard around its strategy. It reaches every budget read off a provider
    /// `P` injects, through any depth of `#[inject]`, and every
    /// [ambient](Budget::ambient) one.
    pub fn around<P: Any>(port: &'static str, wait: Duration) -> Self {
        Self {
            port,
            wait,
            reach: Reach::Around(TypeId::of::<P>()),
        }
    }
}

/// Refuse the first budget at or past a net reaching it, nets and budgets in
/// declaration order, each budget read off the provider `builder` holds —
/// `only` those of the providers it names, or every one. `descriptors` are the
/// modules the app reaches, whose injections a net around a provider follows.
///
/// A resource declared twice — by the module opening it and by a binding over
/// it — is one budget, ambient if either declaration is.
pub(crate) fn check_budgets(
    builder: &ContainerBuilder,
    descriptors: &[&ModuleDescriptor],
    only: Option<&[TypeId]>,
) -> Result<(), BudgetPastNetError> {
    let nets: Vec<&Net> = builder.attached_meta::<Net>().collect();
    if nets.is_empty() {
        return Ok(());
    }
    let mut budgets: Vec<(&Budget, bool)> = Vec::new();
    for budget in builder.attached_meta::<Budget>() {
        match budgets.iter_mut().find(|(seen, _)| {
            seen.provider() == budget.provider() && seen.resource() == budget.resource()
        }) {
            Some((_, ambient)) => *ambient |= budget.is_ambient(),
            None => budgets.push((budget, budget.is_ambient())),
        }
    }
    budgets.retain(|(budget, _)| {
        builder.contains(budget.provider())
            && only.is_none_or(|providers| providers.contains(&budget.provider()))
    });
    if budgets.is_empty() {
        return Ok(());
    }
    let container = builder.snapshot();
    for net in nets {
        let injected = match net.reach {
            Reach::Over(_) => HashSet::new(),
            Reach::Around(provider) => injection_closure(descriptors, provider),
        };
        for (budget, ambient) in &budgets {
            let reached = match net.reach {
                Reach::Over(resource) => budget.provider() == resource,
                Reach::Around(_) => *ambient || injected.contains(&budget.provider()),
            };
            if !reached {
                continue;
            }
            let Some(wait) = budget.wait(&container) else {
                continue;
            };
            if wait >= net.wait {
                return Err(BudgetPastNetError {
                    resource: budget.resource(),
                    budget: wait,
                    port: net.port,
                    net: net.wait,
                    setting: budget.setting().to_owned(),
                });
            }
        }
    }
    Ok(())
}
