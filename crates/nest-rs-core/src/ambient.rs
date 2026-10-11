//! The ambient context: what a unit of work carries for the code it calls, and
//! how far each value travels when the unit continues on another future.
//!
//! The values ride inside the request context's one task-local, a few inline
//! before a spill, so adding one is one allocation and reading one is a scan
//! of a handful of `TypeId`s (`.claude/decisions/ambient-carry.md`).

use std::any::{Any, TypeId};
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use tokio::time::Instant;

use crate::request_scope;

/// A value a unit carries for the code it calls, and how far it travels.
///
/// Implementing it is the visible line that lets a type travel: nothing is
/// ambient until its type says so, a guard's principal type included.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_core::{AmbientValue, ambient, with_ambient};
///
/// struct Tenant(&'static str);
///
/// impl AmbientValue for Tenant {}
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// with_ambient(Arc::new(Tenant("acme")), async {
///     assert_eq!(ambient::<Tenant>().map(|tenant| tenant.0), Some("acme"));
/// })
/// .await;
/// assert!(ambient::<Tenant>().is_none());
/// # }
/// ```
pub trait AmbientValue: Any + Send + Sync {
    /// What crosses into `into`: the value itself (the default, everywhere but
    /// a job's envelope), a narrower value, or nothing.
    fn carry(self: Arc<Self>, into: Continuation) -> Option<Arc<Self>> {
        (into != Continuation::Envelope).then_some(self)
    }
}

/// Where a unit continues once the future that accepted it is no longer the
/// one running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Continuation {
    /// Its streaming response body (`#[sse]`, a download), polled after the
    /// handler returned.
    Body,
    /// A DataLoader batch it triggers, run on another task while it waits.
    /// Shared by every field of the request that loads through the loader, so
    /// it carries the request unit's values, never a nested field's.
    Batch,
    /// A task it hands work to with `TaskContext`, which may outlive it.
    Task,
    /// Work it holds for its commit: `after_commit`, an emitted event's
    /// listeners.
    Commit,
    /// The next units of a connection it opened: a socket's messages, a
    /// graphql-ws operation.
    Connection,
    /// A job it pushes: nothing crosses but the envelope's own keys.
    Envelope,
}

/// The running unit's `T`, when it carries one.
pub fn ambient<T: AmbientValue>() -> Option<Arc<T>> {
    request_scope::current_request_ctx(|ctx| ctx.ambient.get::<T>()).flatten()
}

/// Run `fut` with `value` added to what the running unit carries, replacing
/// the `T` it carried for `fut` alone.
pub fn with_ambient<T: AmbientValue, F: Future>(
    value: Arc<T>,
    fut: F,
) -> impl Future<Output = F::Output> {
    with_ambient_from(move || value, fut)
}

/// [`with_ambient`] with the value `make` returns where `fut` first runs, so it
/// can read what the unit carries there.
pub fn with_ambient_from<T: AmbientValue, F: Future>(
    make: impl FnOnce() -> Arc<T>,
    fut: F,
) -> impl Future<Output = F::Output> {
    request_scope::install(
        move || {
            let mut added = Entries::default();
            added.set(Entry::of(make()));
            added
        },
        fut,
    )
}

/// When the running unit must end by, on tokio's clock, the clock
/// [`UnitContext::deadline`](crate::UnitContext::deadline) reads. A
/// [`Batch`](Continuation::Batch), which the unit waits on, keeps it; every
/// other continuation drops it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline(pub Instant);

impl AmbientValue for Deadline {
    fn carry(self: Arc<Self>, into: Continuation) -> Option<Arc<Self>> {
        (into == Continuation::Batch).then_some(self)
    }
}

/// A value as the map holds it.
type Erased = Arc<dyn Any + Send + Sync>;

/// How one erased value crosses a continuation: its type's own `carry`.
type CarryFn = fn(Erased, Continuation) -> Option<Erased>;

/// One ambient value, with the `carry` of the type it was added as.
#[derive(Clone)]
pub(crate) struct Entry {
    id: TypeId,
    value: Erased,
    carry: CarryFn,
}

impl Entry {
    pub(crate) fn of<T: AmbientValue>(value: Arc<T>) -> Self {
        Self {
            id: TypeId::of::<T>(),
            value,
            carry: carry_erased::<T>,
        }
    }
}

/// `T::carry` behind the erased value, monomorphised where the value was added
/// so the map stays object-safe.
fn carry_erased<T: AmbientValue>(value: Erased, into: Continuation) -> Option<Erased> {
    let typed = value.downcast::<T>().ok()?;
    let carried: Erased = T::carry(typed, into)?;
    Some(carried)
}

/// How many values ride inside the context's own allocation.
const INLINE: usize = 4;

/// The values one unit carries, at most one per type.
#[derive(Clone, Default)]
pub(crate) struct Entries {
    /// Filled front to back, never emptied in place.
    inline: [Option<Entry>; INLINE],
    spill: Vec<Entry>,
}

impl Entries {
    pub(crate) fn get<T: AmbientValue>(&self) -> Option<Arc<T>> {
        let id = TypeId::of::<T>();
        let held = self.iter().find(|held| held.id == id)?;
        Arc::clone(&held.value).downcast::<T>().ok()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inline[0].is_none()
    }

    pub(crate) fn len(&self) -> usize {
        self.iter().count()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.inline.iter().flatten().chain(&self.spill)
    }

    /// Add `entry`, replacing the value of its type.
    pub(crate) fn set(&mut self, entry: Entry) {
        match self
            .inline
            .iter_mut()
            .flatten()
            .chain(&mut self.spill)
            .find(|held| held.id == entry.id)
        {
            Some(held) => *held = entry,
            None => self.push(entry),
        }
    }

    /// Whether `entry`'s very value is already the one held for its type.
    pub(crate) fn holds(&self, entry: &Entry) -> bool {
        self.iter()
            .any(|held| held.id == entry.id && Arc::ptr_eq(&held.value, &entry.value))
    }

    /// What crosses into `into`, each value through its own `carry`; `None`
    /// when every value crosses unchanged.
    pub(crate) fn carried(&self, into: Continuation) -> Option<Self> {
        let mut changed = false;
        let mut crossed = Self::default();
        for entry in self.iter() {
            match (entry.carry)(Arc::clone(&entry.value), into) {
                Some(value) => {
                    changed |= !Arc::ptr_eq(&value, &entry.value);
                    crossed.push(Entry {
                        id: entry.id,
                        value,
                        carry: entry.carry,
                    });
                }
                None => changed = true,
            }
        }
        changed.then_some(crossed)
    }

    /// Add an entry whose type is not held yet.
    fn push(&mut self, entry: Entry) {
        match self.inline.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(entry),
            None => self.spill.push(entry),
        }
    }
}

/// What a stage attached to its unit, made ambient once the whole stage has
/// passed, so a denied unit installs nothing.
#[derive(Clone, Default)]
pub struct Pending(Entries);

impl Pending {
    /// Record `value`, replacing a `T` recorded earlier.
    pub fn attach<T: AmbientValue>(&mut self, value: Arc<T>) {
        self.0.set(Entry::of(value));
    }

    /// The `T` recorded, if any.
    pub fn get<T: AmbientValue>(&self) -> Option<Arc<T>> {
        self.0.get::<T>()
    }

    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending")
            .field("values", &self.0.len())
            .finish()
    }
}

/// Run `fut` with everything `pending` recorded made ambient, in one install.
pub fn install_pending<F: Future>(pending: Pending, fut: F) -> impl Future<Output = F::Output> {
    request_scope::install(move || pending.0, fut)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::request_scope::{RequestContinuation, with_request_scope};
    use crate::{Correlation, TaskContext, current_trace_id};

    /// A package's value, crossing with the default `carry`.
    #[derive(Debug, PartialEq)]
    struct Tenant(&'static str);

    impl AmbientValue for Tenant {}

    /// A value that narrows on the way out: a handle stepping down to its pool.
    #[derive(Debug, PartialEq)]
    enum Handle {
        Transaction,
        Pool,
    }

    impl AmbientValue for Handle {
        fn carry(self: Arc<Self>, into: Continuation) -> Option<Arc<Self>> {
            match into {
                Continuation::Body | Continuation::Batch | Continuation::Commit => {
                    Some(Arc::new(Handle::Pool))
                }
                _ => None,
            }
        }
    }

    const EVERY: [Continuation; 6] = [
        Continuation::Body,
        Continuation::Batch,
        Continuation::Task,
        Continuation::Commit,
        Continuation::Connection,
        Continuation::Envelope,
    ];

    /// What a `T` reads as on the far side of `into`, captured inside `fut`'s
    /// values.
    async fn across<T: AmbientValue>(value: Arc<T>, into: Continuation) -> Option<Arc<T>> {
        let carried = with_request_scope(None, Correlation::minted(None), async {
            with_ambient(value, async { RequestContinuation::capture(into) }).await
        })
        .await
        .expect("a unit is running where it was captured");
        carried.enter(ambient::<T>)
    }

    #[tokio::test]
    async fn a_value_is_read_where_it_was_added_and_nowhere_else() {
        assert!(ambient::<Tenant>().is_none(), "no unit, no value");
        let seen = with_ambient(Arc::new(Tenant("acme")), async { ambient::<Tenant>() }).await;
        assert_eq!(seen.as_deref(), Some(&Tenant("acme")));
        assert!(
            ambient::<Tenant>().is_none(),
            "the value goes with its scope"
        );
    }

    #[tokio::test]
    async fn a_nested_value_shadows_the_outer_one_and_restores_it() {
        with_ambient(Arc::new(Tenant("outer")), async {
            let inner =
                with_ambient(Arc::new(Tenant("inner")), async { ambient::<Tenant>() }).await;
            assert_eq!(inner.as_deref(), Some(&Tenant("inner")));
            assert_eq!(ambient::<Tenant>().as_deref(), Some(&Tenant("outer")));
        })
        .await;
    }

    #[tokio::test]
    async fn an_install_built_as_an_argument_merges_into_what_runs_around_it() {
        let correlation = Correlation::minted(None);
        let trace = correlation.trace_id();
        let inner = with_ambient(Arc::new(Handle::Transaction), async {
            (current_trace_id(), ambient::<Tenant>(), ambient::<Handle>())
        });
        let unit = with_request_scope(
            None,
            correlation,
            with_ambient(Arc::new(Tenant("acme")), inner),
        );
        let (seen, tenant, handle) = unit.await;
        assert_eq!(seen, Some(trace));
        assert_eq!(tenant.as_deref(), Some(&Tenant("acme")));
        assert_eq!(handle.as_deref(), Some(&Handle::Transaction));
    }

    #[tokio::test]
    async fn adding_a_value_keeps_the_units_correlation_and_its_other_values() {
        let correlation = Correlation::minted(None);
        let trace = correlation.trace_id();
        with_request_scope(None, correlation, async {
            with_ambient(Arc::new(Tenant("acme")), async {
                with_ambient(Arc::new(Handle::Transaction), async {
                    assert_eq!(current_trace_id(), Some(trace));
                    assert_eq!(ambient::<Tenant>().as_deref(), Some(&Tenant("acme")));
                    assert_eq!(ambient::<Handle>().as_deref(), Some(&Handle::Transaction));
                })
                .await;
            })
            .await;
        })
        .await;
    }

    #[tokio::test]
    async fn more_values_than_ride_inline_spill_and_stay_readable() {
        struct A;
        struct B;
        struct C;
        struct D;
        struct E;
        impl AmbientValue for A {}
        impl AmbientValue for B {}
        impl AmbientValue for C {}
        impl AmbientValue for D {}
        impl AmbientValue for E {}

        let mut pending = Pending::default();
        pending.attach(Arc::new(A));
        pending.attach(Arc::new(B));
        pending.attach(Arc::new(C));
        pending.attach(Arc::new(D));
        pending.attach(Arc::new(E));
        pending.attach(Arc::new(Tenant("sixth")));
        install_pending(pending, async {
            assert!(ambient::<A>().is_some() && ambient::<D>().is_some());
            assert!(ambient::<E>().is_some(), "the fifth value spilled");
            assert_eq!(ambient::<Tenant>().as_deref(), Some(&Tenant("sixth")));
        })
        .await;
    }

    #[tokio::test]
    async fn a_default_value_crosses_every_continuation_but_a_jobs_envelope() {
        for into in EVERY {
            let crossed = across(Arc::new(Tenant("acme")), into).await;
            match into {
                Continuation::Envelope => assert!(crossed.is_none(), "{into:?}"),
                _ => assert_eq!(crossed.as_deref(), Some(&Tenant("acme")), "{into:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_narrowing_value_crosses_as_what_its_carry_returns() {
        for into in EVERY {
            let crossed = across(Arc::new(Handle::Transaction), into).await;
            match into {
                Continuation::Body | Continuation::Batch | Continuation::Commit => {
                    assert_eq!(crossed.as_deref(), Some(&Handle::Pool), "{into:?}");
                }
                _ => assert!(crossed.is_none(), "{into:?} drops it"),
            }
        }
    }

    #[tokio::test]
    async fn a_deadline_crosses_into_a_batch_alone() {
        let deadline = Deadline(Instant::now() + Duration::from_secs(5));
        for into in EVERY {
            let crossed = across(Arc::new(deadline), into).await;
            match into {
                Continuation::Batch => assert_eq!(crossed.as_deref(), Some(&deadline)),
                _ => assert!(crossed.is_none(), "{into:?} drops it"),
            }
        }
    }

    #[tokio::test]
    async fn a_capture_takes_what_the_unit_carries_when_it_is_captured() {
        let carried = with_ambient(Arc::new(Tenant("acme")), async {
            TaskContext::capture(Continuation::Task)
        })
        .await;
        assert!(ambient::<Tenant>().is_none());
        let seen = tokio::spawn(carried.carry(async { ambient::<Tenant>() }))
            .await
            .expect("the task runs");
        assert_eq!(seen.as_deref(), Some(&Tenant("acme")));
    }

    #[tokio::test]
    async fn a_nested_units_value_never_reaches_a_batch() {
        let request = Correlation::minted(None);
        let request_trace = request.trace_id();
        let request_span = request.span_id();
        let batch = with_request_scope(None, request.clone(), async {
            with_ambient(Arc::new(Handle::Transaction), async {
                with_request_scope(None, request.child(), async {
                    with_ambient(Arc::new(Tenant("the field's own")), async {
                        assert_eq!(
                            ambient::<Tenant>().as_deref(),
                            Some(&Tenant("the field's own"))
                        );
                        RequestContinuation::capture(Continuation::Batch)
                    })
                    .await
                })
                .await
            })
            .await
        })
        .await
        .expect("captured inside a unit");

        batch.enter(|| {
            assert!(
                ambient::<Tenant>().is_none(),
                "the field's attachment stays with the field",
            );
            assert_eq!(
                ambient::<Handle>().as_deref(),
                Some(&Handle::Pool),
                "the request's own value crosses, through its carry",
            );
            assert_eq!(current_trace_id(), Some(request_trace));
            assert_eq!(
                crate::current_span_id(),
                Some(request_span),
                "a batch is the request's, not the field's",
            );
            assert!(
                crate::current_request_scope().is_none(),
                "a batch opens no request scope"
            );
        });
    }

    #[tokio::test]
    async fn a_nested_unit_inherits_what_its_parent_carries() {
        with_ambient(Arc::new(Tenant("acme")), async {
            let seen = with_request_scope(None, Correlation::minted(None), async {
                ambient::<Tenant>()
            })
            .await;
            assert_eq!(seen.as_deref(), Some(&Tenant("acme")));
        })
        .await;
    }

    #[tokio::test]
    async fn a_pending_value_is_ambient_only_once_installed() {
        let mut pending = Pending::default();
        pending.attach(Arc::new(Tenant("acme")));
        assert_eq!(pending.get::<Tenant>().as_deref(), Some(&Tenant("acme")));
        assert!(ambient::<Tenant>().is_none(), "recorded is not installed");
        let seen = install_pending(pending, async { ambient::<Tenant>() }).await;
        assert_eq!(seen.as_deref(), Some(&Tenant("acme")));
    }

    #[tokio::test]
    async fn adding_the_value_already_carried_installs_nothing_new() {
        let tenant = Arc::new(Tenant("acme"));
        with_ambient(Arc::clone(&tenant), async {
            let before = RequestContinuation::capture(Continuation::Task).expect("installed");
            with_ambient(Arc::clone(&tenant), async {
                let after = RequestContinuation::capture(Continuation::Task).expect("installed");
                assert!(after.is_same(&before), "the same value reuses the context");
            })
            .await;
        })
        .await;
    }
}
