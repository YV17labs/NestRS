//! [`Attempting`] — the policies whose attempt this thread is polling now: a
//! run opened inside one of them, directly or through a call of another
//! policy, is that attempt's own call, let through by nothing again and
//! counted once, by the attempt it is made in.

use std::cell::{Cell, RefCell};

use crate::mechanisms::Mechanisms;

/// The depth of nested attempts a thread holds with no allocation.
const INLINE: usize = 8;

/// The policies whose attempt's call one thread is polling, outermost first:
/// the first [`INLINE`] in place, so a run allocates nothing to say so, and
/// any deeper behind them.
struct Polling {
    inline: [Cell<*const Mechanisms>; INLINE],
    deeper: RefCell<Vec<*const Mechanisms>>,
    len: Cell<usize>,
}

impl Polling {
    #[inline]
    fn push(&self, policy: *const Mechanisms) -> usize {
        let len = self.len.get();
        match self.inline.get(len) {
            Some(free) => free.set(policy),
            None => self.deeper.borrow_mut().push(policy),
        }
        self.len.set(len + 1);
        len
    }

    fn contains(&self, policy: *const Mechanisms) -> bool {
        let len = self.len.get();
        self.inline[..len.min(INLINE)]
            .iter()
            .any(|held| held.get() == policy)
            || (len > INLINE && self.deeper.borrow().contains(&policy))
    }

    #[inline]
    fn truncate(&self, len: usize) {
        if self.len.get() > INLINE {
            self.deeper
                .borrow_mut()
                .truncate(len.saturating_sub(INLINE));
        }
        self.len.set(len);
    }
}

thread_local! {
    static ATTEMPTING: Polling = const {
        Polling {
            inline: [const { Cell::new(std::ptr::null()) }; INLINE],
            deeper: RefCell::new(Vec::new()),
            len: Cell::new(0),
        }
    };
}

/// One poll of an attempt's call under the policy it holds, for as long as it
/// lasts: a panic out of the poll ends it as it unwinds.
pub(crate) struct Attempting {
    /// How many policies the thread was polling an attempt of already.
    around: usize,
}

impl Attempting {
    /// The poll of an attempt of `policy`'s.
    #[inline]
    pub(crate) fn enter(policy: &Mechanisms) -> Self {
        let around = ATTEMPTING.with(|attempting| attempting.push(policy));
        Self { around }
    }

    /// Whether this thread is polling an attempt of `policy`'s now.
    #[inline]
    pub(crate) fn inside(policy: &Mechanisms) -> bool {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING.with(|attempting| attempting.contains(pointer))
    }
}

impl Drop for Attempting {
    #[inline]
    fn drop(&mut self) {
        ATTEMPTING.with(|attempting| attempting.truncate(self.around));
    }
}
