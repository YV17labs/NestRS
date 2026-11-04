//! The in-band guard stage: each guard's [`Guard::check`] on a unit an in-band
//! edge carries, then the posture's class gate on the same unit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_core::__private::{ClassGate, ResolvedLayer, run_layer};
use nest_rs_core::{AmbientValue, Continuation, UnitContext};

use crate::denial::{by_cause, denial_of};
use crate::line::file_denial;
use crate::{Denial, Guard};

/// Why a guard stage refused its unit.
#[derive(Debug)]
#[non_exhaustive]
pub enum Refused {
    /// A guard's denial, beside the guard that raised it: the edge files it, on
    /// the target the guard declares, and renders it.
    Guard {
        /// The guard's name, the line's `guard` field.
        guard: &'static str,
        /// The target the guard declares ([`Guard::target`]).
        target: &'static str,
        /// The denial, as raised.
        denial: Denial,
    },
    /// A denial its decider filed — the posture's class gate: the edge renders
    /// it alone, so one denial is one line.
    Filed(Denial),
}

impl Refused {
    /// `denial`, raised by `guard` under the name `name`, read by cause: the
    /// principal it requires, absent, is the caller's.
    pub fn by(name: &'static str, guard: &dyn Guard, denial: Denial) -> Self {
        Self::Guard {
            guard: name,
            target: guard.target(),
            denial: by_cause(denial, guard),
        }
    }

    /// The denial to render, a guard's filed first as `message` on the unit
    /// `cx`: the one line its denial files, on the target the guard declares.
    pub fn into_denial(self, message: &'static str, cx: &UnitContext<'_>) -> Denial {
        match self {
            Self::Guard {
                guard,
                target,
                denial,
            } => {
                file_denial(message, guard, target, cx, &denial);
                denial
            }
            Self::Filed(denial) => denial,
        }
    }
}

/// Ambient on a unit whose in-band units refusing their caller must mark it —
/// the HTTP request carrying a GraphQL or an MCP operation — so a layer around
/// that unit tells a refusal its status line does not show: `Server-Timing`
/// times no refusal. It crosses into what those units run on: a GraphQL
/// batch, and the task rmcp runs an MCP operation on.
#[derive(Clone, Debug, Default)]
pub struct RefusalMark(Arc<AtomicBool>);

impl RefusalMark {
    /// A guard stage of a unit inside refused its caller.
    pub fn mark(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether a guard stage of a unit inside refused its caller.
    pub fn is_marked(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl AmbientValue for RefusalMark {
    fn carry(self: Arc<Self>, into: Continuation) -> Option<Arc<Self>> {
        matches!(into, Continuation::Batch | Continuation::Task).then_some(self)
    }
}

/// Run every guard of `chain` on the unit `cx` describes through
/// [`Guard::check`], in order, then the posture's class `gate`, leaving what
/// they attached on `cx` for the pipeline to install once its guard stage
/// passed. The first refusal stops them.
pub async fn check_guards(
    chain: &[ResolvedLayer<dyn Guard>],
    gate: Option<ClassGate>,
    cx: &mut UnitContext<'_>,
) -> Result<(), Refused> {
    for entry in chain {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        let guard = entry.layer.as_ref();
        run_layer(cx, Some(entry.name));
        let checked = guard.check(cx).await;
        run_layer(cx, None);
        checked.map_err(|denial| Refused::by(entry.name, guard, denial))?;
    }
    if let Some(gate) = gate {
        gate(cx).map_err(|refused| Refused::Filed(denial_of(&refused)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{Layer, MissingAmbientError, async_trait};

    use super::*;
    use crate::PrincipalClaim;

    struct Principal;
    struct Tenant;

    /// Requires [`Principal`], or does without it.
    struct Requiring(fn() -> PrincipalClaim);

    impl Layer for Requiring {}

    #[async_trait]
    impl Guard for Requiring {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }

        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some((self.0)())
        }
    }

    fn refused<T: 'static>(guard: &Requiring) -> Denial {
        let raised = MissingAmbientError::of::<T>().into();
        let Refused::Guard { denial, .. } = Refused::by("guard", guard, raised) else {
            panic!("a guard's denial is a guard's");
        };
        denial
    }

    #[test]
    fn the_principal_a_guard_requires_absent_is_the_callers_401() {
        let denial = refused::<Principal>(&Requiring(PrincipalClaim::of::<Principal>));
        assert_eq!(denial.http_status(), 401);
        assert_eq!(denial.reason(), Some(crate::denial::ANONYMOUS_CALLER));
    }

    /// The guard deciding a visitor itself, reaching this path all the same, is
    /// wiring it never wrote; another value's absence is no caller's doing.
    #[test]
    fn any_other_missing_value_stays_the_wiring_500() {
        let doing_without = Requiring(|| PrincipalClaim::of::<Principal>().or_visitor());
        assert_eq!(refused::<Principal>(&doing_without).http_status(), 500);

        let requiring = Requiring(PrincipalClaim::of::<Principal>);
        assert_eq!(refused::<Tenant>(&requiring).http_status(), 500);
    }

    /// The units a request carries run on a batch (GraphQL) or a task of their
    /// own (MCP); nothing else the request hands on marks it.
    #[test]
    fn a_refusal_mark_crosses_into_a_batch_and_a_task_alone() {
        for (into, crosses) in [
            (Continuation::Batch, true),
            (Continuation::Task, true),
            (Continuation::Body, false),
            (Continuation::Commit, false),
            (Continuation::Connection, false),
            (Continuation::Envelope, false),
        ] {
            let carried = Arc::new(RefusalMark::default()).carry(into);
            assert_eq!(carried.is_some(), crosses, "{into:?}");
        }
    }
}
