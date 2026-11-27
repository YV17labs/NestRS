//! The meter provider the observability stack seats as the global one,
//! counted each time it does: what registered instruments, or made a counter,
//! on the provider before reads the count to register again on the one seated
//! since, with no lock and no read of the provider itself.

use std::sync::atomic::{AtomicU64, Ordering};

static SEATED: AtomicU64 = AtomicU64::new(0);

pub(crate) mod __private {
    use super::{Ordering, SEATED};

    /// A meter provider seated as the global one, called once it is.
    pub fn meter_provider_seated() {
        SEATED.fetch_add(1, Ordering::Release);
    }

    /// How many meter providers the observability stack seated so far: one
    /// read changing since the last means instruments made then sit on a
    /// provider no longer the global one.
    #[inline]
    pub fn meter_providers_seated() -> u64 {
        SEATED.load(Ordering::Acquire)
    }
}
