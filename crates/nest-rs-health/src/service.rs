use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use nest_rs_core::{Container, ReachableProviders, injectable, inventory};

use crate::config::HealthConfig;
use crate::indicator::{HealthIndicator, IndicatorReport, IndicatorStatus, ProbeKind, ProbeReport};

/// The reason a body carries for a check that failed. Fixed and opaque:
/// `/health/*` is routinely unauthenticated, so the error chain goes to the
/// `warn` instead.
const REASON_FAILED: &str = "check failed";

/// The reason a body carries for an indicator that outran its own ceiling.
const REASON_TIMED_OUT: &str = "timed out";

/// The reason a body carries for an indicator that had not answered when the
/// **probe** deadline elapsed.
const REASON_DEADLINE: &str = "probe deadline exceeded";

/// Aggregates every reachable [`HealthIndicator`] submitted via `#[indicators]`
/// into a per-probe [`ProbeReport`]; a probe with zero indicators reports `up`.
#[injectable]
#[derive(Default)]
pub struct HealthService {
    /// Set once at bootstrap by `HealthModule`.
    container: OnceLock<Container>,
    /// Absent without `ConfigModule` (a hand-built container): the defaults
    /// stand, so a probe never runs unbounded.
    config: OnceLock<Arc<HealthConfig>>,
}

impl HealthService {
    pub(crate) fn install_container(&self, container: Container) {
        if self.container.set(container.clone()).is_ok() {
            if let Some(config) = container.get::<HealthConfig>() {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "guarded by the container's once-set above; a re-run keeps the first config"
                )]
                let _ = self.config.set(config);
            }
            report_unreachable_indicators(&container);
            // Inside the once-guard: `init()` is re-runnable.
            crate::controller::report_prefixed_probe_paths(&container);
        }
    }

    /// Run every reachable indicator for `kind` **concurrently** and aggregate
    /// their results into a [`ProbeReport`], under the per-indicator ceiling and
    /// the probe deadline of [`HealthConfig`]. Reports `up` if called before
    /// bootstrap wires the container, so a probe racing startup does not flap.
    pub async fn probe(&self, kind: ProbeKind) -> ProbeReport {
        let Some(container) = self.container.get() else {
            return ProbeReport::empty_up();
        };

        // Silent on skips: `report_unreachable_indicators` named them at boot.
        let entries: Vec<&'static HealthIndicator> = reachable_indicators(container)
            .filter(|entry| entry.kind == kind)
            .collect();
        if entries.is_empty() {
            return ProbeReport::empty_up();
        }

        let config = self.config.get().cloned().unwrap_or_default();
        run_indicators(&entries, container, kind, &config).await
    }
}

/// Run `entries` concurrently under both ceilings and fold them into a report.
async fn run_indicators(
    entries: &[&HealthIndicator],
    container: &Container,
    kind: ProbeKind,
    config: &HealthConfig,
) -> ProbeReport {
    let indicator_timeout = config.indicator_timeout();
    // Not `spawn`ed: the operation span and trace context are task-locals, and
    // a spawned check would file its `warn` under no unit of work.
    let mut running: FuturesUnordered<_> = entries
        .iter()
        .enumerate()
        .map(|(slot, entry)| async move {
            let outcome =
                run_with_timeout(entry.name, kind, (entry.run)(container), indicator_timeout).await;
            (slot, outcome)
        })
        .collect();

    let deadline = tokio::time::Instant::now() + config.probe_deadline();
    let mut outcomes: Vec<Option<(IndicatorStatus, Option<String>)>> = vec![None; entries.len()];
    loop {
        match tokio::time::timeout_at(deadline, running.next()).await {
            Ok(Some((slot, outcome))) => outcomes[slot] = Some(outcome),
            Ok(None) => break,
            Err(_elapsed) => {
                let unanswered = outcomes.iter().filter(|slot| slot.is_none()).count();
                tracing::warn!(
                    target: crate::TARGET,
                    ?kind,
                    deadline_ms = config.probe_deadline_ms,
                    answered = outcomes.len() - unanswered,
                    unanswered,
                    "health probe deadline exceeded",
                );
                break;
            }
        }
    }

    ProbeReport::from_indicators(
        entries
            .iter()
            .zip(outcomes)
            .map(|(entry, outcome)| {
                let (status, error) =
                    outcome.unwrap_or((IndicatorStatus::Down, Some(REASON_DEADLINE.to_owned())));
                IndicatorReport {
                    name: entry.name,
                    status,
                    error,
                }
            })
            .collect(),
    )
}

/// Two reachable indicators claiming one name on one probe is a **boot**
/// failure naming both hosts: [`ProbeReport::from_indicators`] folds by name,
/// so a `down` verdict could be overwritten by an `up` one.
///
/// Per probe, not per registry: `nest-rs-seaorm` ships one `db` on readiness
/// and one on startup.
pub(crate) fn check_indicator_names(container: &Container) -> anyhow::Result<()> {
    check_names(reachable_indicators(container))
}

/// Every linked indicator this app can actually run; shared by the boot check
/// and the probe, which must agree.
///
/// No [`ReachableProviders`] means a hand-built container: every indicator is
/// in scope.
fn reachable_indicators(
    container: &Container,
) -> impl Iterator<Item = &'static HealthIndicator> + use<> {
    let reachable = container.get::<ReachableProviders>();
    inventory::iter::<HealthIndicator>().filter(move |entry| {
        reachable
            .as_ref()
            .is_none_or(|r| r.0.contains(&(entry.provider_type_id)()))
    })
}

/// The registry-free half of [`check_indicator_names`]: `inventory` is
/// process-wide, so tests hand the entries in.
fn check_names<'a>(entries: impl Iterator<Item = &'a HealthIndicator>) -> anyhow::Result<()> {
    let mut claimed: std::collections::HashMap<(ProbeKind, &'static str), &'static str> =
        std::collections::HashMap::new();
    for entry in entries {
        if let Some(first) = claimed.insert((entry.kind, entry.name), entry.origin) {
            anyhow::bail!(
                "duplicate health indicator {name:?} on the {kind:?} probe: {first} and \
                 {second} both claim it. That name is the probe body's JSON key, so one \
                 verdict would silently replace the other — rename one of the two methods",
                name = entry.name,
                kind = entry.kind,
                second = entry.origin,
            );
        }
    }
    Ok(())
}

/// Name the linked-but-unreachable indicators **once, at boot**; probing then
/// skips in silence.
fn report_unreachable_indicators(container: &Container) {
    let Some(reachable) = container.get::<ReachableProviders>() else {
        return;
    };
    for entry in inventory::iter::<HealthIndicator>() {
        if !reachable.0.contains(&(entry.provider_type_id)()) {
            report_inert_indicator(container, entry);
        }
    }
}

/// Report an inert indicator at the level its owner earns: `debug` for a
/// framework capability the app never opted into, `warn` for its own code.
fn report_inert_indicator(container: &Container, entry: &HealthIndicator) {
    ::nest_rs_core::report_inert_host!(
        target: crate::TARGET,
        what: "indicator",
        origin: entry.origin,
        host: (entry.provider_type_id)(),
        container: container,
        indicator = entry.name,
        kind = ::tracing::field::debug(entry.kind),
    );
}

/// Run one indicator future under a wall-clock ceiling, mapping success,
/// failure, and timeout to a `(status, error)` pair with an opaque reason.
async fn run_with_timeout(
    name: &'static str,
    kind: ProbeKind,
    fut: impl std::future::Future<Output = anyhow::Result<()>>,
    timeout: Duration,
) -> (IndicatorStatus, Option<String>) {
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(())) => (IndicatorStatus::Up, None),
        Ok(Err(err)) => {
            tracing::warn!(
                target: crate::TARGET,
                indicator = name,
                ?kind,
                error = %nest_rs_core::error_message(&*err),
                "health indicator failed",
            );
            (IndicatorStatus::Down, Some(REASON_FAILED.to_owned()))
        }
        Err(_elapsed) => {
            tracing::warn!(
                target: crate::TARGET,
                indicator = name,
                ?kind,
                timeout_ms = timeout.as_millis(),
                "health indicator timed out",
            );
            (IndicatorStatus::Down, Some(REASON_TIMED_OUT.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicator::IndicatorRun;
    use nest_rs_core::Container;

    #[tokio::test]
    async fn a_hanging_indicator_times_out_to_down() {
        let logs = nest_rs_testing::LogCapture::install();
        let (status, error) = run_with_timeout(
            "hang",
            ProbeKind::Readiness,
            std::future::pending::<anyhow::Result<()>>(),
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(status, IndicatorStatus::Down);
        assert_eq!(
            error.as_deref(),
            Some(REASON_TIMED_OUT),
            "a timed-out indicator reports Down with an opaque reason",
        );

        let event = logs.expect_one(crate::TARGET, "health indicator timed out");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("indicator").as_deref(), Some("hang"));
        assert_eq!(event.field("timeout_ms").as_deref(), Some("10"));
        assert!(
            event.field("kind").is_some_and(|k| k.contains("Readiness")),
            "the event names the probe that hung, got {:?}",
            event.fields,
        );
    }

    #[test]
    fn a_framework_owned_indicator_reports_at_debug() {
        let logs = nest_rs_testing::LogCapture::install();
        // Not submitted: `inventory` is process-wide.
        report_inert_indicator(
            &Container::builder().build(),
            &HealthIndicator {
                origin: "nest_rs_seaorm::health::indicator",
                name: "db",
                kind: ProbeKind::Readiness,
                provider_type_id: || std::any::TypeId::of::<UpHost>(),
                run: |_| Box::pin(async move { Ok(()) }),
            },
        );

        let skipped = logs.find(crate::TARGET, "skipped indicator: not this app's to run");
        assert_eq!(skipped.len(), 1, "one line: {:#?}", logs.events());
        assert_eq!(skipped[0].level, "debug");
        assert_eq!(skipped[0].field("indicator").as_deref(), Some("db"));
        assert_eq!(skipped[0].field("cause").as_deref(), Some("framework"));
    }

    #[tokio::test]
    async fn a_fast_indicator_is_not_affected_by_the_ceiling() {
        let (status, error) = run_with_timeout(
            "ok",
            ProbeKind::Readiness,
            async { Ok(()) },
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(status, IndicatorStatus::Up);
        assert!(error.is_none());
    }

    struct UpHost;
    struct DownHost;

    impl UpHost {
        async fn ping(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }
    impl DownHost {
        async fn ping(&self) -> anyhow::Result<()> {
            anyhow::bail!("simulated outage")
        }
    }

    nest_rs_core::inventory::submit! {
        HealthIndicator {
            // Not `module_path!()`: `nest_rs_health::…` would read as the
            // framework's and report at `debug`.
            origin: "features::probes::up",
            name: "up_host",
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<UpHost>(),
            run: |c| Box::pin(async move {
                c.get::<UpHost>().expect("UpHost registered").ping().await
            }),
        }
    }

    nest_rs_core::inventory::submit! {
        HealthIndicator {
            origin: "features::probes::down",
            name: "down_host",
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<DownHost>(),
            run: |c| Box::pin(async move {
                c.get::<DownHost>().expect("DownHost registered").ping().await
            }),
        }
    }

    #[tokio::test]
    async fn aggregates_indicators_into_info_and_error_buckets() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            .build();
        let svc = HealthService::default();
        svc.install_container(container);

        let report = svc.probe(ProbeKind::Readiness).await;
        assert_eq!(report.status, IndicatorStatus::Down);
        assert_eq!(report.info.len(), 1);
        assert!(report.info.contains_key("up_host"));
        assert_eq!(report.error.len(), 1);
        let down = report
            .error
            .get("down_host")
            .expect("down_host in error bucket");
        assert_eq!(down.status, IndicatorStatus::Down);
        assert_eq!(
            down.error.as_deref(),
            Some(REASON_FAILED),
            "public probe responses must not leak indicator internals",
        );
        assert_eq!(report.details.len(), 2);
    }

    #[tokio::test]
    async fn the_indicators_own_error_reaches_the_log_and_never_the_body() {
        let logs = nest_rs_testing::LogCapture::install();
        let (status, error) = run_with_timeout(
            "migrations",
            ProbeKind::Startup,
            async { anyhow::bail!("pending migrations") },
            Duration::from_secs(5),
        )
        .await;

        assert_eq!(status, IndicatorStatus::Down);
        assert_eq!(
            error.as_deref(),
            Some(REASON_FAILED),
            "an unauthenticated probe body carries a fixed reason, never a DSN \
             or a hostname the anyhow chain picked up",
        );

        let event = logs.expect_one(crate::TARGET, "health indicator failed");
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("error").as_deref(),
            Some("pending migrations"),
            "…and the detail is one filtered log query away: {event:#?}",
        );
        assert_eq!(event.field("indicator").as_deref(), Some("migrations"));
    }

    #[tokio::test]
    async fn an_unreachable_indicator_is_named_once_at_boot_not_per_probe() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            .provide(ReachableProviders(
                [std::any::TypeId::of::<UpHost>()].into_iter().collect(),
            ))
            .build();

        let logs = nest_rs_testing::LogCapture::install();
        let svc = HealthService::default();
        svc.install_container(container);

        let skipped = logs.find(
            crate::TARGET,
            "skipped indicator: no instance of the provider in this app's container",
        );
        assert_eq!(skipped.len(), 1, "one line at boot: {:#?}", logs.events());
        assert_eq!(skipped[0].level, "warn");
        assert_eq!(skipped[0].field("indicator").as_deref(), Some("down_host"));

        let report = svc.probe(ProbeKind::Readiness).await;
        let _ = svc.probe(ProbeKind::Readiness).await;
        assert_eq!(report.status, IndicatorStatus::Up);
        assert_eq!(
            logs.find(
                crate::TARGET,
                "skipped indicator: no instance of the provider in this app's container",
            )
            .len(),
            1,
            "probing must not repeat the boot notice: {:#?}",
            logs.events(),
        );
    }

    #[test]
    fn two_hosts_claiming_one_name_on_one_probe_fail_the_boot() {
        let ok: IndicatorRun = |_| Box::pin(async { Ok(()) });
        let mine = HealthIndicator {
            origin: "features::billing::health",
            ..entry("db", ok)
        };
        let theirs = HealthIndicator {
            origin: "nest_rs_seaorm::health::indicator",
            ..entry("db", ok)
        };
        let err = check_names([&mine, &theirs].into_iter())
            .expect_err("one name, one probe, two hosts must not boot");
        let sentence = format!("{err:#}");
        for named in [
            "features::billing::health",
            "nest_rs_seaorm::health::indicator",
            "db",
        ] {
            assert!(
                sentence.contains(named),
                "the boot error names {named}: {sentence}"
            );
        }
    }

    #[test]
    fn one_name_on_two_probes_is_not_a_collision() {
        let ok: IndicatorRun = |_| Box::pin(async { Ok(()) });
        let ready = entry("db", ok);
        let startup = HealthIndicator {
            kind: ProbeKind::Startup,
            ..entry("db", ok)
        };
        check_names([&ready, &startup].into_iter())
            .expect("two probes never share a report, so the name is claimed once each");
    }

    #[test]
    fn distinct_names_on_one_probe_boot() {
        let ok: IndicatorRun = |_| Box::pin(async { Ok(()) });
        let (a, b) = (entry("db", ok), entry("cache", ok));
        check_names([&a, &b].into_iter()).expect("no name is claimed twice");
    }

    /// Built by hand, not submitted: `inventory` is process-wide.
    fn entry(name: &'static str, run: crate::indicator::IndicatorRun) -> HealthIndicator {
        HealthIndicator {
            origin: "features::probes::fixture",
            name,
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<UpHost>(),
            run,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn indicators_run_concurrently_so_the_probe_costs_the_slowest_one() {
        let container = Container::builder().build();
        let config = HealthConfig::default()
            .with_indicator_timeout(Duration::from_secs(30))
            .with_probe_deadline(Duration::from_secs(60));
        let slow: IndicatorRun = |_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(())
            })
        };
        let entries = [
            entry("a", slow),
            entry("b", slow),
            entry("c", slow),
            entry("d", slow),
        ];
        let entries: Vec<&HealthIndicator> = entries.iter().collect();

        let started = tokio::time::Instant::now();
        let report = run_indicators(&entries, &container, ProbeKind::Readiness, &config).await;
        let elapsed = started.elapsed();

        assert_eq!(report.status, IndicatorStatus::Up);
        assert_eq!(report.details.len(), 4);
        assert!(
            elapsed < Duration::from_secs(10),
            "four 5s checks must cost one interval, not four — took {elapsed:?}",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_probe_deadline_bounds_a_check_that_would_outlive_the_kubelet() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = Container::builder().build();
        let config = HealthConfig::default()
            .with_indicator_timeout(Duration::from_secs(300))
            .with_probe_deadline(Duration::from_millis(900));
        let entries = [
            entry("fast", |_| Box::pin(async { Ok(()) })),
            entry("hang", |_| {
                Box::pin(std::future::pending::<anyhow::Result<()>>())
            }),
        ];
        let entries: Vec<&HealthIndicator> = entries.iter().collect();

        let started = tokio::time::Instant::now();
        let report = run_indicators(&entries, &container, ProbeKind::Readiness, &config).await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the response is bounded by the deadline, not by the indicator ceiling",
        );

        assert_eq!(report.status, IndicatorStatus::Down);
        assert!(
            report.info.contains_key("fast"),
            "what answered is still reported: {report:#?}",
        );
        let hung = report
            .error
            .get("hang")
            .expect("the unanswered check is down");
        assert_eq!(
            hung.error.as_deref(),
            Some(REASON_DEADLINE),
            "a fixed, opaque reason — an unauthenticated body carries nothing else",
        );

        let event = logs.expect_one(crate::TARGET, "health probe deadline exceeded");
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("deadline_ms").as_deref(), Some("900"));
        assert_eq!(event.field("answered").as_deref(), Some("1"));
        assert_eq!(event.field("unanswered").as_deref(), Some("1"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_report_is_ordered_by_name_not_by_completion() {
        let container = Container::builder().build();
        let config = HealthConfig::default().with_probe_deadline(Duration::from_secs(60));
        let entries = [
            entry("zulu", |_| Box::pin(async { Ok(()) })),
            entry("alpha", |_| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(())
                })
            }),
        ];
        let entries: Vec<&HealthIndicator> = entries.iter().collect();

        let report = run_indicators(&entries, &container, ProbeKind::Readiness, &config).await;
        let body = serde_json::to_string(&report).expect("the report serializes");
        assert!(
            body.find("\"alpha\"") < body.find("\"zulu\""),
            "keys are name-ordered even though `zulu` finished first: {body}",
        );
    }

    #[tokio::test]
    async fn other_probes_ignore_readiness_indicators() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            .build();
        let svc = HealthService::default();
        svc.install_container(container);

        let report = svc.probe(ProbeKind::Liveness).await;
        assert_eq!(report.status, IndicatorStatus::Up);
        assert!(report.details.is_empty());
    }
}
