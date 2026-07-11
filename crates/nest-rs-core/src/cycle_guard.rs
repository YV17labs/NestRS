//! Shared re-entrancy / cycle detector for provider resolution.
//!
//! Two provider kinds recurse while building — a **transient**
//! ([`container`](crate::container)) and a **request-scoped** provider
//! ([`request_scope`](crate::request_scope)) — and each must turn a provider
//! that (transitively) depends on itself into a clear panic naming the cycle,
//! instead of unbounded recursion. The mechanism is identical for both: a
//! thread-local stack of the types currently being built, a push that detects a
//! re-entry and renders the chain (`A → B → A`), and an RAII guard whose `Drop`
//! pops the entry (including on panic unwind) so a panicking factory never
//! leaves a stale entry that poisons the next resolution on the thread.
//!
//! Each kind keeps its **own** thread-local stack (so a genuine concurrent
//! resolution of the same provider on different threads is never mistaken for a
//! cycle, and the panic wording stays kind-specific); this module owns only the
//! push/pop/render logic they share.

use std::any::TypeId;
use std::cell::RefCell;
use std::thread::LocalKey;

/// A thread-local build stack: the `(TypeId, type name)` pairs of the providers
/// currently under construction on this thread. The `&'static str` companion is
/// captured at push time so the panic diagnostic can render the full chain, not
/// just the type currently being built.
pub(crate) type BuildStack = RefCell<Vec<(TypeId, &'static str)>>;

/// Signals a resolution cycle at `push` time, carrying the rendered chain
/// (`A → B → A`) so the caller panics with the full path, not just the outer
/// type. Stays an internal recoverable signal so each caller supplies its own
/// kind-specific panic message.
pub(crate) struct Cycle {
    pub chain: String,
}

/// RAII drop guard over a [`BuildStack`]: pushes on construction, pops on drop
/// (including panic unwind). Without the pop-on-drop a panicking factory would
/// leave its entry permanently on the stack, poisoning every later resolution
/// of the same provider on that thread with a spurious cycle diagnostic.
pub(crate) struct CycleGuard {
    stack: &'static LocalKey<BuildStack>,
    id: TypeId,
}

impl CycleGuard {
    /// Push `(id, type_name)` onto `stack`, or return [`Cycle`] if `id` is
    /// already present — the second entry for a type closes a cycle.
    pub(crate) fn push(
        stack: &'static LocalKey<BuildStack>,
        id: TypeId,
        type_name: &'static str,
    ) -> Result<Self, Cycle> {
        stack.with(|cell| {
            let mut s = cell.borrow_mut();
            if let Some(start) = s.iter().position(|(sid, _)| *sid == id) {
                // The cycle path is every entry from the first occurrence up to
                // the top of the stack, plus the offender being re-entered.
                let mut names: Vec<&'static str> = s[start..].iter().map(|(_, n)| *n).collect();
                names.push(type_name);
                return Err(Cycle {
                    chain: names.join(" → "),
                });
            }
            s.push((id, type_name));
            Ok(())
        })?;
        Ok(Self { stack, id })
    }
}

impl Drop for CycleGuard {
    fn drop(&mut self) {
        // `rposition` + `swap_remove` rather than `pop`: stays correct even if a
        // future change interleaves entries on the same thread (a factory
        // recursing into a *different* provider of the same kind).
        self.stack.with(|cell| {
            let mut s = cell.borrow_mut();
            if let Some(pos) = s.iter().rposition(|(sid, _)| *sid == self.id) {
                s.swap_remove(pos);
            }
        });
    }
}
