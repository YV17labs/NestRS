use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};
use nest_rs_core::{Container, ReachableProviders, injectable, inventory};

use crate::config::HealthConfig;
use crate::indicator::{HealthIndicator, IndicatorReport, IndicatorStatus, ProbeKind, ProbeReport};

/// The reason a body carries for a check that failed. Fixed and opaque:
/// `/health/*` is routinely unauthenticated, so an `anyhow` chain — a DSN, an
/// internal hostname, a driver message — never reaches it. The chain goes to
/// the `warn` instead.
const REASON_FAILED: &str = "check failed";

/// The reason a body carries for an indicator that outran its own ceiling.
const REASON_TIMED_OUT: &str = "timed out";

/// The reason a body carries for an indicator that had not answered when the
/// **probe** deadline elapsed. Distinct from [`REASON_TIMED_OUT`] because the
/// two say different things to whoever reads the body: one indicator was slow,
/// or the response as a whole ran out of time. Both are constants, so neither
/// can carry anything the caller did not already know.
const REASON_DEADLINE: &str = "probe deadline exceeded";

/// Aggregates every reachable [`HealthIndicator`] submitted via `#[indicators]`
/// into a per-probe [`ProbeReport`]. Apps don't usually touch this directly —
/// they register indicators and the crate's probe controller consumes the
/// reports.
///
/// A probe with zero indicators reports `up` with an empty body: importing
/// only `HealthModule` keeps the default permissive answer Kubernetes expects
/// before any custom check is wired in.
#[injectable]
#[derive(Default)]
pub struct HealthService {
    /// Set once at [`OnApplicationBootstrap`][1] by `HealthModule` so the
    /// service can resolve the per-indicator providers at probe time. The
    /// container is `Clone` (Arcs internally), so this carries a cheap handle.
    ///
    /// [1]: nest_rs_core::LifecyclePhase::OnApplicationBootstrap
    container: OnceLock<Container>,
    /// Resolved at the same phase, from the same container. Absent means the
    /// service was built without `ConfigModule` (a hand-built container in a
    /// test), and the defaults stand — a probe never runs unbounded because a
    /// config failed to resolve.
    config: OnceLock<Arc<HealthConfig>>,
}

impl HealthService {
    pub(crate) fn install_container(&self, container: Container) {
        // `Container` is a cheap `Arc` handle, so the clone is for the report's
        // borrow, not a second container.
        if self.container.set(container.clone()).is_ok() {
            if let Some(config) = container.get::<HealthConfig>() {
                let _ = self.config.set(config);
            }
            report_unreachable_indicators(&container);
            // Inside the once-guard with it, and for the same reason: both are
            // startup facts about this app's wiring, and `init()` is re-runnable
            // (a suite may drive the phases again). Said twice, a boot notice
            // reads as two apps.
            crate::controller::report_prefixed_probe_paths(&container);
        }
    }

    /// Run every reachable indicator for `kind` **concurrently** and aggregate
    /// their results into a [`ProbeReport`]. Reports `up` if called before
    /// bootstrap wires the container, so a probe racing startup does not flap.
    ///
    /// **Two ceilings, and they answer different questions.** The per-indicator
    /// one names the slow check in a `warn` and reports it `down` on its own.
    /// The probe deadline bounds the response *whatever the indicator count is*
    /// — the reason the run is concurrent in the first place: serially, four
    /// indicators at a five-second ceiling were a twenty-second worst case
    /// against a kubelet whose `timeoutSeconds` defaults to **1**, and a
    /// kubelet that gives up scores the probe as failed with nothing logged at
    /// this end, because the ceiling it outlived had not fired yet.
    pub async fn probe(&self, kind: ProbeKind) -> ProbeReport {
        let Some(container) = self.container.get() else {
            // Called before bootstrap — no indicators can run; report `up`
            // so a probe that races the framework's wire-up does not flap.
            return ProbeReport::empty_up();
        };

        // Silent about what it skips on purpose: `report_unreachable_indicators`
        // named it at boot, so repeating per probe would be the same event said
        // twice — once per request, in production.
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
///
/// Extracted from [`HealthService::probe`] for the reason `run_with_timeout` is:
/// `inventory` is process-wide, so a fixture submitted to exercise the deadline
/// would join every other probe in the suite. Here the entries are handed in.
async fn run_indicators(
    entries: &[&HealthIndicator],
    container: &Container,
    kind: ProbeKind,
    config: &HealthConfig,
) -> ProbeReport {
    let indicator_timeout = config.indicator_timeout();
    // Every indicator is an independent `&self` check and nothing orders them,
    // so the probe's cost is the slowest one rather than their sum. They run on
    // **this** task rather than through `spawn`: the operation span and the
    // request's trace context are task-locals, and a spawned check would file
    // its `warn` under no unit of work at all.
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
    // The drained stream is the only termination condition: how many checks
    // are outstanding is a property of `outcomes`, so counting them in
    // lock-step with the writes would be a second spelling of the same fact —
    // and the one the deadline `warn` reports.
    loop {
        match tokio::time::timeout_at(deadline, running.next()).await {
            Ok(Some((slot, outcome))) => outcomes[slot] = Some(outcome),
            Ok(None) => break,
            Err(_elapsed) => {
                // The probe's own half of HEALTH-I7, and the one the body
                // cannot say: which probe ran out of time, against what
                // deadline, and how many checks never answered. An operator
                // reading a flapping `503` has nothing else.
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

    // Folded in `entries` order into `BTreeMap`s keyed by name, so the body an
    // operator diffs between two calls is ordered by the indicator's name and
    // never by which check happened to finish first.
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
/// failure naming both hosts.
///
/// The name is the addressable unit — the JSON key in the probe body and the
/// `indicator` field on every line this crate emits — and
/// [`ProbeReport::from_indicators`] folds by it. So a collision does not merely
/// shadow an entry: a `down` verdict can be overwritten by an `up` one, and a
/// failing check disappears from a readiness probe with nothing said. Which of
/// the two wins is `inventory` link order, which nobody declared.
///
/// This is the one failure mode a merging surface introduces, and the framework
/// answers it the same way everywhere it merges — `nest-rs-mcp`'s duplicate tool
/// name inside an endpoint is the same check for the same reason.
///
/// **Per probe, not per registry**: two kinds never appear in one report, so
/// `#[readiness] fn db` beside `#[startup] fn db` addresses nothing twice — the
/// pair `nest-rs-seaorm` ships is deliberately of that shape.
pub(crate) fn check_indicator_names(container: &Container) -> anyhow::Result<()> {
    check_names(reachable_indicators(container))
}

/// Every linked indicator this app can actually run.
///
/// The fail-open branch is the decision, and it is one decision: no
/// [`ReachableProviders`] means the access graph never ran — a hand-built
/// container in a test — so every indicator is in scope rather than none.
/// Spelled once because the boot check and the probe must agree; disagreeing,
/// the boot would refuse a name collision between two indicators no probe would
/// ever run.
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

/// The registry-free half of [`check_indicator_names`], extracted for the reason
/// [`run_indicators`] and [`run_with_timeout`] are: `inventory` is process-wide,
/// so a fixture submitted to exercise a collision would collide in every other
/// test in the process too. Here the entries are handed in.
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

/// Name the linked-but-unreachable indicators **once, at boot** — the same
/// contract every other discovery seam honours (`nest_rs::queue`,
/// `nest_rs::events`): leftover code never vanishes silently, and the notice is
/// a startup fact rather than a line per probe. Called from
/// `install_container` because that is where the reachability set first exists;
/// probing then skips in silence.
fn report_unreachable_indicators(container: &Container) {
    // No reachability set means the access graph never ran (a hand-built
    // container in a test) — every indicator is then in scope, not skipped.
    let Some(reachable) = container.get::<ReachableProviders>() else {
        return;
    };
    for entry in inventory::iter::<HealthIndicator>() {
        if !reachable.0.contains(&(entry.provider_type_id)()) {
            report_inert_indicator(entry);
        }
    }
}

/// Report an inert indicator at the level its owner earns.
///
/// `debug` for a `nest-rs-*` capability the app never opted into — the
/// developer cannot act on it, and `nest_rs_seaorm`'s `db` / `db_ready` pair
/// warned twice per boot on two shipped demo apps, telling the reader to go
/// bind a framework-internal type. `warn` for the app's own leftover code,
/// which is what the module-gated discovery rule wants seen. Same call the
/// lifecycle runner makes, from the same place.
fn report_inert_indicator(entry: &HealthIndicator) {
    ::nest_rs_core::report_inert_host!(
        target: crate::TARGET,
        what: "indicator",
        origin: entry.origin,
        indicator = entry.name,
        kind = ::tracing::field::debug(entry.kind),
    );
}

/// Run one indicator future under a wall-clock ceiling, mapping success,
/// failure, and timeout to a `(status, error)` pair. Extracted so the timeout
/// branch (HEALTH-I7) is testable without an inventory indicator. Public
/// probe responses carry only opaque reasons — never the indicator's internals.
async fn run_with_timeout(
    name: &'static str,
    kind: ProbeKind,
    fut: impl std::future::Future<Output = anyhow::Result<()>>,
    timeout: Duration,
) -> (IndicatorStatus, Option<String>) {
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(())) => (IndicatorStatus::Up, None),
        Ok(Err(err)) => {
            let detail = format!("{err:#}");
            tracing::warn!(
                target: crate::TARGET,
                indicator = name,
                ?kind,
                error = %detail,
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
    //! Drives the inventory-aggregation path through the crate-private
    //! `install_container` seam — the equivalent of the wired-up bootstrap
    //! hook but without booting an app.

    use super::*;
    use crate::indicator::IndicatorRun;
    use nest_rs_core::Container;

    #[tokio::test]
    async fn a_hanging_indicator_times_out_to_down() {
        // HEALTH-I7: an indicator probing a dead peer must not hang the probe.
        // A tiny real ceiling keeps the test fast while exercising the timeout
        // branch against a never-resolving future.
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

        // The report says `Down` with an opaque reason — deliberately, since a
        // readiness body is served to whatever can reach the endpoint. So which
        // indicator hung, on which probe, and against what ceiling exists only
        // here, and an operator diagnosing a flapping readiness check has
        // nothing else to read.
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

    /// A `nest-rs-*` indicator the app never opted into reports at `debug`, not
    /// `warn`. `nest_rs_seaorm`'s `db` / `db_ready` pair warned twice on every
    /// boot of two shipped demo apps, telling the reader to bind a
    /// framework-internal type. Asserted on the emitted level rather than on
    /// `is_framework_owned`, which `nest-rs-core` already covers — this is the
    /// half only the health branch can be wrong about.
    #[test]
    fn a_framework_owned_indicator_reports_at_debug() {
        let logs = nest_rs_testing::LogCapture::install();
        // Driven directly rather than through a submitted fixture: `inventory`
        // is process-wide, so a second entry would join every other test in
        // this file.
        report_inert_indicator(&HealthIndicator {
            origin: "nest_rs_seaorm::health::indicator",
            name: "db",
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<UpHost>(),
            run: |_| Box::pin(async move { Ok(()) }),
        });

        let skipped = logs.find(
            crate::TARGET,
            "skipped indicator: framework capability not imported by this app",
        );
        assert_eq!(skipped.len(), 1, "one line: {:#?}", logs.events());
        assert_eq!(skipped[0].level, "debug");
        assert_eq!(skipped[0].field("indicator").as_deref(), Some("db"));
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
            // App-shaped, deliberately: `module_path!()` here is
            // `nest_rs_health::…`, which `is_framework_owned` reads as the
            // framework's and reports at `debug`. These fixtures stand in for a
            // developer's own indicator, so they must say so.
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

    /// The two halves of a failed check, and where each is allowed to appear.
    /// They disagreed once — the body promised the operator a stringified
    /// error, the code shipped `"check failed"` — and the resolution is that
    /// `/health/*` is routinely unauthenticated, so the detail belongs in the
    /// log and nowhere else. Both directions are pinned: a future change that
    /// widens the body, *or* one that drops the log line, fails here.
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

    /// A linked-but-unreachable indicator is named **once, at boot** — not on
    /// every probe. The line is a startup fact about the module tree; repeating
    /// it per request turns a wiring notice into production log volume, and no
    /// other discovery seam does that.
    #[tokio::test]
    async fn an_unreachable_indicator_is_named_once_at_boot_not_per_probe() {
        let container = Container::builder()
            .provide(UpHost)
            .provide(DownHost)
            // `DownHost` is deliberately absent from the reachable set: linked
            // into the binary, in no module the running app imports.
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

        // Two probes later, still one line — and the unreachable indicator
        // never ran, so the probe is `up`.
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

    /// Two hosts claiming one name on one probe fail the boot, and the message
    /// names **both** so the reader knows which two to look at. Without this the
    /// fold keeps whichever `inventory` linked last, and a `down` verdict can be
    /// replaced by an `up` one — a failing check leaving a readiness probe with
    /// nothing said anywhere.
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

    /// The check is per **probe**, not per registry: two kinds never appear in
    /// one report, so nothing is addressed twice. This is the shape
    /// `nest-rs-seaorm` ships — one `db` on readiness, one on startup would both
    /// be reachable and neither collides — and a registry-wide check would refuse
    /// the framework's own indicator.
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

    /// Distinct names on one probe are the ordinary case and must stay silent.
    #[test]
    fn distinct_names_on_one_probe_boot() {
        let ok: IndicatorRun = |_| Box::pin(async { Ok(()) });
        let (a, b) = (entry("db", ok), entry("cache", ok));
        check_names([&a, &b].into_iter()).expect("no name is claimed twice");
    }

    /// One indicator entry, built by hand rather than submitted: `inventory` is
    /// process-wide, so a fixture for these tests would run on every other
    /// probe in this file.
    fn entry(name: &'static str, run: crate::indicator::IndicatorRun) -> HealthIndicator {
        HealthIndicator {
            origin: "features::probes::fixture",
            name,
            kind: ProbeKind::Readiness,
            provider_type_id: || std::any::TypeId::of::<UpHost>(),
            run,
        }
    }

    /// Four indicators, each sleeping the same interval, cost **one** interval
    /// rather than four. Serially they were the sum, which is how four checks
    /// at a five-second ceiling became a twenty-second worst case against a
    /// kubelet deadline that defaults to one second. Time is paused, so the
    /// assertion is on the virtual clock and cannot flake on a loaded runner.
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

    /// The probe deadline bounds the response whatever the per-indicator
    /// ceiling says, and what did answer is still reported. Without it a probe
    /// outlives the kubelet's `timeoutSeconds` — scored a failure, with no
    /// `health indicator timed out` line, because the per-indicator ceiling has
    /// not fired yet.
    #[tokio::test(start_paused = true)]
    async fn the_probe_deadline_bounds_a_check_that_would_outlive_the_kubelet() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = Container::builder().build();
        // The ceiling deliberately far above the deadline: this is the shape
        // that used to answer nothing at all.
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

    /// The body's key order is the indicators' names, whatever order they
    /// finished in — an operator diffing two probe responses reads a stable
    /// document rather than a reshuffle.
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
