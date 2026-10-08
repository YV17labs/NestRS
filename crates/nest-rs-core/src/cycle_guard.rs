//! Re-entrancy detector for transient and request-scoped provider resolution:
//! a provider depending on itself panics naming the chain (`A → B → A`) instead
//! of recursing forever. Each kind keeps its own thread-local stack.

use std::any::TypeId;
use std::cell::RefCell;
use std::thread::LocalKey;

/// The `(TypeId, type name)` pairs of the providers under construction on this
/// thread.
pub(crate) type BuildStack = RefCell<Vec<(TypeId, &'static str)>>;

/// A resolution cycle, with its rendered chain; each caller words its own panic.
pub(crate) struct Cycle {
    pub chain: String,
}

/// Pushes on construction and pops on drop, including on unwind: a panicking
/// factory must not leave an entry that reads as a cycle on the next resolution.
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
        self.stack.with(|cell| {
            let mut s = cell.borrow_mut();
            if let Some(pos) = s.iter().rposition(|(sid, _)| *sid == self.id) {
                s.swap_remove(pos);
            }
        });
    }
}
