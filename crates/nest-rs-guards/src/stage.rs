//! The in-band guard stage: each guard's [`Guard::check`] on a unit an in-band
//! edge carries, then the posture's class gate on the same unit.

use nest_rs_core::__private::{ClassGate, Pending, ResolvedLayer, take_pending};
use nest_rs_core::UnitContext;

use crate::denial::denial_of;
use crate::{Denial, Guard};

/// Why a guard stage refused its unit.
#[derive(Debug)]
pub enum Refused {
    /// A guard's denial nobody filed, beside the name of the guard that raised
    /// it, as raised: the edge files it and renders it.
    Guard(&'static str, Denial),
    /// A denial its decider filed — the posture's class gate, or a guard that
    /// [files its own](Guard::files_its_denials): the edge renders it alone,
    /// so one denial is one line.
    Filed(Denial),
}

impl Refused {
    /// `denial`, raised by `guard` under the name `name`.
    pub fn by(name: &'static str, guard: &dyn Guard, denial: Denial) -> Self {
        match guard.files_its_denials() {
            true => Self::Filed(denial),
            false => Self::Guard(name, denial),
        }
    }

    /// The denial to render, a guard's nobody filed passed first to `file`, the
    /// edge's structural floor.
    pub fn into_denial(self, file: impl FnOnce(&'static str, &Denial)) -> Denial {
        match self {
            Self::Guard(guard, denial) => {
                file(guard, &denial);
                denial
            }
            Self::Filed(denial) => denial,
        }
    }
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
        let guard = entry.layer.as_ref();
        guard
            .check(cx)
            .await
            .map_err(|denial| Refused::by(entry.name, guard, denial))?;
    }
    if let Some(gate) = gate {
        gate(cx).map_err(|refused| Refused::Filed(denial_of(&refused)))?;
    }
    Ok(take_pending(cx))
}
