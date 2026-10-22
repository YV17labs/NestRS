use std::borrow::Cow;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use nest_rs_core::{
    Container, Discovery, ReachableProviders, Unanswered, injectable, inventory, within,
};

use crate::config::HealthConfig;
use crate::indicator::{
    HealthIndicator, IndicatorEntry, IndicatorReport, IndicatorRun, IndicatorStatus, ProbeKind,
    ProbeReport,
};

/// The reason a body carries for a check that failed. Fixed and opaque:
/// `/health/*` is routinely unauthenticated, so the error chain goes to the
/// `warn` instead.
const REASON_FAILED: &str = "check failed";

/// The reason a body carries for an indicator that outran its own ceiling.
const REASON_TIMED_OUT: &str = "timed out";

/// The reason a body carries for an indicator that had not answered when the
/// **probe** deadline elapsed.
const REASON_DEADLINE: &str = "probe deadline exceeded";

/// Aggregates every indicator this app runs — each reachable one
/// `#[indicators]` submitted, and each [`IndicatorEntry`] an imported module
/// attached — into a per-probe [`ProbeReport`]; a probe with zero indicators
/// reports `up`.
#[injectable]
#[derive(Default)]
pub struct HealthService {
    /// Set by `HealthModule`'s wiring step, before the first hook, and taken
    /// back by its close: held for good, it would keep the container, and
    /// every singleton in it, alive past the app.
    installed: RwLock<Option<Installed>>,
    /// Absent without `ConfigModule` (a hand-built container): the defaults
    /// stand, so a probe never runs unbounded.
    config: OnceLock<Arc<HealthConfig>>,
}

/// What the wiring hands the service: the container the checks run against,
/// and the indicators the boot's name check passed.
#[derive(Clone)]
struct Installed {
    container: Container,
    indicators: Arc<[Indicator]>,
}

/// One indicator this app runs, whichever registry it came from — the one the
/// probe runs and the boot's name check reads, so the two agree.
pub(crate) struct Indicator {
    name: Cow<'static, str>,
    kind: ProbeKind,
    /// Where it was declared: the decorated method's module path, or the type
    /// of the module that attached it.
    origin: &'static str,
    run: IndicatorRun,
}

impl From<&HealthIndicator> for Indicator {
    fn from(entry: &HealthIndicator) -> Self {
        Self {
            name: Cow::Borrowed(entry.name),
            kind: entry.kind,
            origin: entry.origin,
            run: entry.run,
        }
    }
}

impl From<&IndicatorEntry> for Indicator {
    fn from(entry: &IndicatorEntry) -> Self {
        Self {
            name: entry.name.clone(),
            kind: entry.kind,
            origin: entry.origin,
            run: entry.run,
        }
    }
}

/// What the indicator registry holds, as a refusal to read it ungated names it.
pub(crate) const INDICATORS: &str = "indicator";

/// Every indicator `container`'s app runs: the ones `#[indicators]` submitted
/// on a provider `reachable` holds, then the entries its modules attached —
/// gated by construction, since only an imported module registers.
pub(crate) fn wired(container: &Container, reachable: &ReachableProviders) -> Vec<Indicator> {
    inventory::iter::<HealthIndicator>()
        .filter(|entry| reachable.reaches((entry.provider_type_id)()))
        .map(Indicator::from)
        .chain(
            Discovery::new(container)
                .meta::<IndicatorEntry>()
                .iter()
                .map(|attached| Indicator::from(&*attached.meta)),
        )
        .collect()
}

impl HealthService {
    /// Hold `container` and the `indicators` its probes run, once, naming
    /// those linked on a provider `reachable` does not hold; a second install
    /// keeps the first.
    pub(crate) fn install(
        &self,
        container: &Container,
        reachable: &ReachableProviders,
        indicators: Vec<Indicator>,
    ) {
        let installed = {
            let mut held = self
                .installed
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            let empty = held.is_none();
            if empty {
                *held = Some(Installed {
                    container: container.clone(),
                    indicators: indicators.into(),
                });
            }
            empty
        };
        if installed {
            if let Some(config) = container.get::<HealthConfig>() {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "guarded by the container's once-set above; a second install keeps the first config"
                )]
                let _ = self.config.set(config);
            }
            report_unreachable_indicators(container, reachable);
            // Inside the guard: a service two live apps share reports once.
            crate::controller::report_prefixed_probe_paths(container);
        }
    }

    /// Hand the container back once the app ends, so its singletons drop.
    pub(crate) fn release_container(&self) {
        self.installed
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    /// Run every indicator this app runs for `kind` **concurrently** and
    /// aggregate their results into a [`ProbeReport`], under the per-indicator
    /// ceiling and the probe deadline of [`HealthConfig`]. The boot's wiring
    /// step hands it the container before any hook runs; a service no boot
    /// wired (a hand-built container) reports `up` with no indicator run.
    pub async fn probe(&self, kind: ProbeKind) -> ProbeReport {
        let held = self
            .installed
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(Installed {
            container,
            indicators,
        }) = held
        else {
            return ProbeReport::empty_up();
        };

        let entries: Vec<&Indicator> = indicators
            .iter()
            .filter(|entry| entry.kind == kind)
            .collect();
        if entries.is_empty() {
            return ProbeReport::empty_up();
        }

        let config = self.config.get().cloned().unwrap_or_default();
        run_indicators(&entries, &container, kind, &config).await
    }
}

/// Run `entries` concurrently under both ceilings and fold them into a report.
async fn run_indicators(
    entries: &[&Indicator],
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
            // In the bounded call, so an indicator panicking before it hands
            // back its future is contained too.
            let check = async { (entry.run)(container).await };
            let outcome = run_with_timeout(&entry.name, kind, check, indicator_timeout).await;
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
                    name: entry.name.clone(),
                    status,
                    error,
                }
            })
            .collect(),
    )
}

/// Two indicators claiming one name on one probe is a **boot** failure naming
/// both origins: [`ProbeReport::from_indicators`] folds by name, so a `down`
/// verdict could be overwritten by an `up` one.
///
/// Per probe, not per registry: `nest-rs-seaorm` ships one `seaorm` on
/// readiness and one on startup.
pub(crate) fn check_names<'a>(
    entries: impl IntoIterator<Item = &'a Indicator>,
) -> anyhow::Result<()> {
    let mut claimed: std::collections::HashMap<(ProbeKind, &'a str), &'static str> =
        std::collections::HashMap::new();
    for entry in entries {
        if let Some(first) = claimed.insert((entry.kind, entry.name.as_ref()), entry.origin) {
            anyhow::bail!(
                "duplicate health indicator {name:?} on the {kind:?} probe: {first} and \
                 {second} both claim it. That name is the probe body's JSON key, so one \
                 verdict would silently replace the other — rename one of the two methods, or \
                 drop the check a resource's health binding already runs",
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
fn report_unreachable_indicators(container: &Container, reachable: &ReachableProviders) {
    for entry in inventory::iter::<HealthIndicator>() {
        if !reachable.reaches((entry.provider_type_id)()) {
            report_inert_indicator(container, entry);
        }
    }
}

/// Report an inert indicator at the level its owner earns: `debug` for a
/// dependency's host the app does not import, `warn` for its own code.
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
/// failure, a panic and timeout to a `(status, error)` pair with an opaque
/// reason.
async fn run_with_timeout(
    name: &str,
    kind: ProbeKind,
    fut: impl std::future::Future<Output = anyhow::Result<()>>,
    timeout: Duration,
) -> (IndicatorStatus, Option<String>) {
    match within(timeout, fut).await {
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
        Err(Unanswered::Panicked(payload)) => {
            nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload.as_ref(),
                "health indicator panicked",
                indicator = name,
                kind = ?kind,
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

    #[nest_rs_core::module]
    struct NoIndicatorModule;

    /// The framework's indicator is read off the composition like any
    /// dependency's, never off its crate's name.
    #[tokio::test]
    async fn a_dependencys_indicator_this_app_does_not_import_reports_at_debug() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = nest_rs_core::App::builder()
            .module::<NoIndicatorModule>()
            .build()
            .await
            .expect("an empty module boots");
        // Not submitted: `inventory` is process-wide.
        report_inert_indicator(
            app.container(),
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
        assert_eq!(skipped[0].field("cause").as_deref(), Some("dependency"));
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

    /// Install what `container`'s set reaches, as the wiring step does.
    fn install(svc: &HealthService, container: &Container) {
        let reachable =
            ReachableProviders::of(container, INDICATORS).expect("the test seeds the set");
        svc.install(container, &reachable, wired(container, &reachable));
    }

    /// The set reaching both hosts.
    fn reaching_both() -> ReachableProviders {
        nest_rs_core::__private::reachable_providers([
            std::any::TypeId::of::<UpHost>(),
            std::any::TypeId::of::<DownHost>(),
        ])
    }

    #[tokio::test]
    async fn aggregates_indicators_into_info_and_error_buckets() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            .provide(reaching_both())
            .build()
            .expect("the container builds");
        let svc = HealthService::default();
        install(&svc, &container);

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
            .provide(nest_rs_core::__private::reachable_providers([
                std::any::TypeId::of::<UpHost>(),
            ]))
            .build()
            .expect("the container builds");

        let logs = nest_rs_testing::LogCapture::install();
        let svc = HealthService::default();
        install(&svc, &container);

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
        let mine = Indicator {
            origin: "features::billing::health",
            ..entry("seaorm", ok)
        };
        let theirs = Indicator {
            origin: "nest_rs_seaorm::health::module::SeaOrmHealthModule",
            ..entry("seaorm", ok)
        };
        let err = check_names([&mine, &theirs])
            .expect_err("one name, one probe, two hosts must not boot");
        let sentence = format!("{err:#}");
        for named in [
            "features::billing::health",
            "nest_rs_seaorm::health::module::SeaOrmHealthModule",
            "seaorm",
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
        let ready = entry("seaorm", ok);
        let startup = Indicator {
            kind: ProbeKind::Startup,
            ..entry("seaorm", ok)
        };
        check_names([&ready, &startup])
            .expect("two probes never share a report, so the name is claimed once each");
    }

    #[test]
    fn distinct_names_on_one_probe_boot() {
        let ok: IndicatorRun = |_| Box::pin(async { Ok(()) });
        let (a, b) = (entry("seaorm", ok), entry("redis:cache", ok));
        check_names([&a, &b]).expect("no name is claimed twice");
    }

    /// Built by hand, not submitted: `inventory` is process-wide.
    fn entry(name: &'static str, run: IndicatorRun) -> Indicator {
        Indicator {
            name: Cow::Borrowed(name),
            kind: ProbeKind::Readiness,
            origin: "features::probes::fixture",
            run,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn indicators_run_concurrently_so_the_probe_costs_the_slowest_one() {
        let container = Container::builder().build().expect("the container builds");
        let config = HealthConfig {
            indicator_timeout_ms: 30_000,
            probe_deadline_ms: 60_000,
        };
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
        let entries: Vec<&Indicator> = entries.iter().collect();

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
        let container = Container::builder().build().expect("the container builds");
        let config = HealthConfig {
            indicator_timeout_ms: 300_000,
            probe_deadline_ms: 900,
        };
        let entries = [
            entry("fast", |_| Box::pin(async { Ok(()) })),
            entry("hang", |_| {
                Box::pin(std::future::pending::<anyhow::Result<()>>())
            }),
        ];
        let entries: Vec<&Indicator> = entries.iter().collect();

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
        let container = Container::builder().build().expect("the container builds");
        let config = HealthConfig {
            probe_deadline_ms: 60_000,
            ..HealthConfig::default()
        };
        let entries = [
            entry("zulu", |_| Box::pin(async { Ok(()) })),
            entry("alpha", |_| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(())
                })
            }),
        ];
        let entries: Vec<&Indicator> = entries.iter().collect();

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
            .provide(reaching_both())
            .build()
            .expect("the container builds");
        let svc = HealthService::default();
        install(&svc, &container);

        let report = svc.probe(ProbeKind::Liveness).await;
        assert_eq!(report.status, IndicatorStatus::Up);
        assert!(report.details.is_empty());
    }
}
