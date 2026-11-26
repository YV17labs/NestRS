//! [`Attempting`] — the policies whose attempt this thread is polling now: a
//! run opened inside one of them, directly or through a call of another
//! policy, is that attempt's own call, let through by nothing again and
//! counted once, by the attempt it is made in.

use std::cell::RefCell;

use crate::mechanisms::Mechanisms;

/// The depth of nested attempts a thread holds with no allocation.
const INLINE: usize = 8;

/// The policies whose attempt's call one thread is polling, outermost first:
/// the first [`INLINE`] in place, so a run allocates nothing to say so, and
/// any deeper behind them.
struct Polling {
    inline: [*const Mechanisms; INLINE],
    deeper: Vec<*const Mechanisms>,
    len: usize,
}

impl Polling {
    fn push(&mut self, policy: *const Mechanisms) {
        match self.inline.get_mut(self.len) {
            Some(free) => *free = policy,
            None => self.deeper.push(policy),
        }
        self.len += 1;
    }

    fn contains(&self, policy: *const Mechanisms) -> bool {
        self.inline[..self.len.min(INLINE)].contains(&policy) || self.deeper.contains(&policy)
    }

    fn truncate(&mut self, len: usize) {
        self.len = len;
        self.deeper.truncate(len.saturating_sub(INLINE));
    }
}

thread_local! {
    static ATTEMPTING: RefCell<Polling> = const {
        RefCell::new(Polling {
            inline: [std::ptr::null(); INLINE],
            deeper: Vec::new(),
            len: 0,
        })
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
        let pointer: *const Mechanisms = policy;
        let around = ATTEMPTING.with_borrow_mut(|attempting| {
            attempting.push(pointer);
            attempting.len - 1
        });
        Self { around }
    }

    /// Whether this thread is polling an attempt of `policy`'s now.
    #[inline]
    pub(crate) fn inside(policy: &Mechanisms) -> bool {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING.with_borrow(|attempting| attempting.contains(pointer))
    }
}

impl Drop for Attempting {
    #[inline]
    fn drop(&mut self) {
        ATTEMPTING.with_borrow_mut(|attempting| attempting.truncate(self.around));
    }
}
