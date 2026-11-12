//! The process's own metrics, as the OpenTelemetry process conventions name
//! them, read when the metric reader exports and never between: its CPU time,
//! its uptime and its open descriptors everywhere, and on Linux, the platform a
//! deployment runs on, its memory and its threads.

use std::io;
use std::sync::Once;
use std::time::Instant;

use opentelemetry::KeyValue;
use opentelemetry::metrics::Meter;
use opentelemetry_semantic_conventions::attribute::CPU_MODE;
use opentelemetry_semantic_conventions::metric::{
    PROCESS_CPU_TIME, PROCESS_UNIX_FILE_DESCRIPTOR_COUNT, PROCESS_UPTIME,
};

use crate::durations::SECONDS;

/// Registers the process's instruments.
pub(crate) struct ProcessMetrics;

impl ProcessMetrics {
    /// Register the process's instruments on `meter`, its uptime counted from
    /// `started`, when telemetry started at the top of `main`.
    pub(crate) fn observe(meter: &Meter, started: Instant) {
        let reported = Once::new();
        meter
            .f64_observable_counter(PROCESS_CPU_TIME)
            .with_unit(SECONDS)
            .with_description("Total CPU seconds broken down by different CPU modes")
            .with_callback(move |observer| match cpu_time() {
                Ok((user, system)) => {
                    observer.observe(user, &[KeyValue::new(CPU_MODE, "user")]);
                    observer.observe(system, &[KeyValue::new(CPU_MODE, "system")]);
                }
                Err(error) => unread(&reported, PROCESS_CPU_TIME, &error),
            })
            .build();
        meter
            .f64_observable_gauge(PROCESS_UPTIME)
            .with_unit(SECONDS)
            .with_description("The time the process has been running.")
            .with_callback(move |observer| observer.observe(started.elapsed().as_secs_f64(), &[]))
            .build();
        let reported = Once::new();
        meter
            .i64_observable_up_down_counter(PROCESS_UNIX_FILE_DESCRIPTOR_COUNT)
            .with_unit("{file_descriptor}")
            .with_description("Number of unix file descriptors in use by the process")
            .with_callback(move |observer| match open_descriptors() {
                Ok(count) => observer.observe(count, &[]),
                Err(error) => unread(&reported, PROCESS_UNIX_FILE_DESCRIPTOR_COUNT, &error),
            })
            .build();
        #[cfg(target_os = "linux")]
        linux::observe(meter);
    }
}

/// Say once, at `warn`, that `metric` cannot be read, and why: its series
/// stops where it stopped.
fn unread(reported: &Once, metric: &'static str, error: &io::Error) {
    reported.call_once(|| {
        tracing::warn!(
            target: crate::TARGET,
            metric,
            error = %nest_rs_core::error_message(error),
            "a process metric cannot be read; its series stops",
        );
    });
}

/// The CPU time the process spent in user and in system mode, in seconds.
fn cpu_time() -> io::Result<(f64, f64)> {
    // SAFETY: `getrusage` only writes the `rusage` it is handed, which is
    // plain data and valid zeroed.
    #[expect(
        unsafe_code,
        reason = "the process's CPU time is read through libc, which std lacks"
    )]
    let (read, usage) = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        (libc::getrusage(libc::RUSAGE_SELF, &mut usage), usage)
    };
    if read != 0 {
        return Err(io::Error::last_os_error());
    }
    // A `timeval` is never negative; its microseconds are an `i32` on macOS
    // and an `i64` on Linux.
    let seconds = |time: libc::timeval| {
        let whole = u64::try_from(time.tv_sec).unwrap_or(0);
        let micros = u32::try_from(time.tv_usec).unwrap_or(0);
        std::time::Duration::new(whole, micros.saturating_mul(1_000)).as_secs_f64()
    };
    Ok((seconds(usage.ru_utime), seconds(usage.ru_stime)))
}

/// How many descriptors the process holds open, the listing's own left out.
fn open_descriptors() -> io::Result<i64> {
    let listed = std::fs::read_dir("/dev/fd")?.count();
    Ok(i64::try_from(listed.saturating_sub(1)).unwrap_or(i64::MAX))
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io;
    use std::sync::Once;

    use opentelemetry::metrics::Meter;
    use opentelemetry_semantic_conventions::metric::{
        PROCESS_MEMORY_USAGE, PROCESS_MEMORY_VIRTUAL, PROCESS_THREAD_COUNT,
    };

    use super::unread;

    /// Register the instruments `/proc/self/status` answers.
    pub(super) fn observe(meter: &Meter) {
        for (metric, field, unit, description, scale) in [
            (
                PROCESS_MEMORY_USAGE,
                "VmRSS",
                "By",
                "The amount of physical memory in use",
                1024,
            ),
            (
                PROCESS_MEMORY_VIRTUAL,
                "VmSize",
                "By",
                "The amount of committed virtual memory",
                1024,
            ),
            (
                PROCESS_THREAD_COUNT,
                "Threads",
                "{thread}",
                "Process threads count",
                1,
            ),
        ] {
            let reported = Once::new();
            meter
                .i64_observable_up_down_counter(metric)
                .with_unit(unit)
                .with_description(description)
                .with_callback(move |observer| match status(field) {
                    Ok(value) => observer.observe(value.saturating_mul(scale), &[]),
                    Err(error) => unread(&reported, metric, &error),
                })
                .build();
        }
    }

    /// The number `/proc/self/status` holds under `field` (`VmRSS:  1234 kB`).
    pub(super) fn status(field: &str) -> io::Result<i64> {
        let status = std::fs::read_to_string("/proc/self/status")?;
        status
            .lines()
            .find_map(|line| {
                let value = line.strip_prefix(field)?.strip_prefix(':')?;
                value.split_whitespace().next()?.parse().ok()
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a field the kernel writes"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_has_spent_cpu_time_and_holds_descriptors() {
        let busy = Instant::now();
        while busy.elapsed() < std::time::Duration::from_millis(20) {}
        let (user, system) = cpu_time().expect("getrusage reads");
        assert!(user + system > 0.0, "{user} {system}");
        assert!(open_descriptors().expect("the descriptors list") > 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_kernel_says_the_processs_memory_and_threads() {
        for field in ["VmRSS", "VmSize", "Threads"] {
            assert!(linux::status(field).expect(field) > 0, "{field}");
        }
    }
}
