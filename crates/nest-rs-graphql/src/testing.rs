//! Test doubles: the clock a test compares a cost on.

/// The CPU time this thread has run. A thread waiting for a core accrues
/// none, so a cost read on it holds however loaded the machine is; the
/// current-thread runtime runs every task of an execution on it.
#[expect(unsafe_code, reason = "std has no thread CPU clock")]
pub(crate) fn thread_cpu_time() -> std::time::Duration {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid `timespec` the call only writes.
    let read = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut now) };
    assert_eq!(read, 0, "the thread's CPU clock reads");
    std::time::Duration::new(
        u64::try_from(now.tv_sec).expect("a CPU time is not negative"),
        u32::try_from(now.tv_nsec).expect("nanoseconds below a second"),
    )
}
