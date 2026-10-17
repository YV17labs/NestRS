//! The in-band guard stage: each guard's [`Guard::check`] on a unit an in-band
//! edge carries, then the posture's class gate on the same unit.

use nest_rs_core::__private::{ClassGate, Pending, ResolvedLayer, take_pending};
use nest_rs_core::UnitContext;

use crate::denial::denial_of;
use crate::{Denial, Guard};

/// Why a guard stage refused its unit.
#[derive(Debug)]
pub enum Refused {
    /// A guard's denial, beside the name of the guard that raised it, as
    /// raised: the edge files it and renders it.
    Guard(&'static str, Denial),
    /// The posture's class gate's denial: the gate filed its own line, so the
    /// edge renders it alone.
    Gate(Denial),
}

/// Run every guard of `chain` on the unit `cx` describes through its in-band
/// entry, [`Guard::check`], in order, then the posture's class `gate` on the
/// same unit: what the guards attached, for the edge to install once the stage
/// passed. The first refusal stops the stage.
pub async fn run_stage(
    chain: &[ResolvedLayer<dyn Guard>],
    gate: Option<ClassGate>,
    cx: &mut UnitContext<'_>,
) -> Result<Pending, Refused> {
    for entry in chain {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        entry
            .layer
            .as_ref()
            .check(cx)
            .await
            .map_err(|denial| Refused::Guard(entry.name, denial))?;
    }
    if let Some(gate) = gate {
        gate(cx).map_err(|refused| Refused::Gate(denial_of(&refused)))?;
    }
    Ok(take_pending(cx))
}
