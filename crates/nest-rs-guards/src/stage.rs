//! The in-band guard stage: each guard's [`Guard::check`] on a unit an in-band
//! edge carries, then the principal its handler needs, then the posture's class
//! gate on the same unit.

use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_core::__private::{ClassGate, ResolvedLayer, carries, withhold_from_reads};
use nest_rs_core::{AmbientValue, Continuation, UnitContext};

use crate::denial::{anonymous_denial, by_cause, denial_of};
use crate::line::file_denial;
use crate::validate::produces;
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

    /// [`into_denial`](Self::into_denial) for a unit run once per parent, a
    /// GraphQL field resolver's: a guard's denial is filed only where `first`
    /// says the operation has yet to file it, named by the guard and its status,
    /// so its count is no caller's to choose.
    pub fn into_denial_once(
        self,
        message: &'static str,
        cx: &UnitContext<'_>,
        first: impl FnOnce(String) -> bool,
    ) -> Denial {
        match self {
            Self::Guard { guard, denial, .. }
                if !first(format!("{guard} {}", denial.http_status())) =>
            {
                denial
            }
            refused => refused.into_denial(message, cx),
        }
    }
}

/// A principal a `#[public]` unit's handler reads as an `Ambient`, beside the
/// guard of its chain or its request that produces it
/// ([`Guard::produced_principal`]): absent once the guards passed, that guard
/// admitted the caller without one, and the caller's `401` is the unit's, filed
/// as that guard's denial, on every edge.
#[derive(Clone)]
pub struct NeededPrincipal {
    principal: TypeId,
    guard: ResolvedLayer<dyn Guard>,
}

impl NeededPrincipal {
    /// On a `#[public]` unit (`public`), the first of the `ambient` types its
    /// handler reads that a guard of `guards` produces as its principal, beside
    /// the first such guard.
    pub fn of<'g, I>(public: bool, ambient: &[TypeId], guards: I) -> Option<Self>
    where
        I: IntoIterator<Item = &'g ResolvedLayer<dyn Guard>>,
        I::IntoIter: Clone,
    {
        if !public {
            return None;
        }
        let guards = guards.into_iter();
        ambient.iter().find_map(|principal| {
            let guard = guards.clone().find(|entry| produces(entry, *principal))?;
            Some(Self {
                principal: *principal,
                guard: guard.clone(),
            })
        })
    }

    /// [`of`](Self::of) over the `chain` of a unit an in-band request carries and
    /// the chain that `request` ran, resolved only for a `#[public]` unit whose
    /// handler reads an `Ambient` at all.
    pub fn carried(
        public: bool,
        ambient: &[TypeId],
        chain: &[ResolvedLayer<dyn Guard>],
        request: impl FnOnce() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
    ) -> Result<Option<Self>, String> {
        if !public || ambient.is_empty() {
            return Ok(None);
        }
        Ok(Self::of(public, ambient, chain.iter().chain(&request()?)))
    }

    /// The guard producing the principal, which a refusal and a document name.
    pub fn guard(&self) -> &ResolvedLayer<dyn Guard> {
        &self.guard
    }

    /// The refusal of the unit `cx` describes, its guards having admitted it
    /// without the principal: the `401` `earned` says a credential set aside on
    /// its way to it earned, else the caller's own.
    pub fn refused(
        &self,
        cx: &UnitContext<'_>,
        earned: impl FnOnce(TypeId) -> Option<Denial>,
    ) -> Option<Refused> {
        if carries(cx, self.principal) {
            return None;
        }
        let denial = earned(self.principal).unwrap_or_else(anonymous_denial);
        Some(Refused::by(
            self.guard.name,
            self.guard.layer.as_ref(),
            denial,
        ))
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
/// [`Guard::check`], in order, then refuse a caller they admitted without the
/// `needed` principal, then the posture's class `gate`, leaving what they
/// attached on `cx` for the pipeline to install once its guard stage passed,
/// `withheld` from the unit's reads where its posture names no subject. The first
/// refusal stops them.
pub async fn check_guards(
    chain: &[ResolvedLayer<dyn Guard>],
    needed: Option<&NeededPrincipal>,
    gate: Option<ClassGate>,
    withheld: bool,
    cx: &mut UnitContext<'_>,
) -> Result<(), Refused> {
    for entry in chain {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        let guard = entry.layer.as_ref();
        guard
            .check(cx)
            .await
            .map_err(|denial| Refused::by(entry.name, guard, denial))?;
    }
    // In band, a credential the request could not verify refused it there.
    if let Some(refused) = needed.and_then(|needed| needed.refused(cx, |_| None)) {
        return Err(refused);
    }
    if let Some(gate) = gate {
        gate(cx).map_err(|refused| Refused::Filed(denial_of(&refused)))?;
    }
    if withheld {
        withhold_from_reads(cx);
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
    #[derive(Layer)]
    struct Requiring(fn() -> PrincipalClaim);

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
