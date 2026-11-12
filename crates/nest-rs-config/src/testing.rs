//! Test doubles: a lock left poisoned, as a holder that panics leaves it.

/// Takes a guard with `take` on a thread that then panics holding it, so the
/// lock the guard came from is poisoned for the rest of the process.
pub(crate) fn panic_holding<G>(take: impl FnOnce() -> G + Send + 'static) {
    let holder = std::thread::spawn(move || {
        let _guard = take();
        panic!("a holder panicked with the lock held");
    });
    assert!(holder.join().is_err(), "the holder panicked");
}
