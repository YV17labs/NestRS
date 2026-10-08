//! Health indicator contract and link-time registry.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;

use nest_rs_core::Container;
use nest_rs_http::schemars::JsonSchema;
use serde::Serialize;

/// Which Kubernetes-style probe an indicator participates in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeKind {
    /// Liveness — is the process healthy enough to keep running?
    Liveness,
    /// Readiness — can the process serve traffic right now?
    Readiness,
    /// Startup — has the process finished initializing?
    Startup,
}

/// `up` when the indicator's check returned `Ok`; `down` otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(crate = "nest_rs_http::schemars")]
pub enum IndicatorStatus {
    /// The check returned `Ok`.
    Up,
    /// The check returned an error.
    Down,
}

/// Outcome of a single indicator check, included in a [`ProbeReport`].
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(crate = "nest_rs_http::schemars")]
pub struct IndicatorReport {
    /// The indicator's stable id (its snake_case method name).
    pub name: &'static str,
    /// Whether this indicator's check passed.
    pub status: IndicatorStatus,
    /// `Some` only when the check failed — a **fixed, opaque** reason
    /// (`"check failed"` / `"timed out"` / `"probe deadline exceeded"`), never
    /// the indicator's own error, which is logged at `warn` on
    /// `nest_rs::health`: `/health/*` is routinely unauthenticated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregated outcome of a probe: an overall `status` (`200` up, `503` down)
/// plus per-indicator reports.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(crate = "nest_rs_http::schemars")]
pub struct ProbeReport {
    /// The overall probe result — `down` if any indicator is down.
    pub status: IndicatorStatus,
    /// Up indicators, keyed by name.
    pub info: std::collections::BTreeMap<&'static str, IndicatorReport>,
    /// Down indicators, keyed by name. Empty when `status == Up`.
    pub error: std::collections::BTreeMap<&'static str, IndicatorReport>,
    /// Every indicator that ran, up or down — the union of `info` and `error`.
    pub details: std::collections::BTreeMap<&'static str, IndicatorReport>,
}

impl ProbeReport {
    pub(crate) fn empty_up() -> Self {
        Self {
            status: IndicatorStatus::Up,
            info: Default::default(),
            error: Default::default(),
            details: Default::default(),
        }
    }

    pub(crate) fn from_indicators(reports: Vec<IndicatorReport>) -> Self {
        let mut info = std::collections::BTreeMap::new();
        let mut error = std::collections::BTreeMap::new();
        let mut details = std::collections::BTreeMap::new();
        let mut status = IndicatorStatus::Up;
        for r in reports {
            match r.status {
                IndicatorStatus::Up => {
                    info.insert(r.name, r.clone());
                }
                IndicatorStatus::Down => {
                    error.insert(r.name, r.clone());
                    status = IndicatorStatus::Down;
                }
            }
            details.insert(r.name, r);
        }
        Self {
            status,
            info,
            error,
            details,
        }
    }
}

/// The boxed future one indicator check resolves to, borrowing the [`Container`].
pub type IndicatorFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// The thunk stored in [`HealthIndicator::run`]; a function pointer because
/// `inventory` submits it from a `const` context.
pub type IndicatorRun = for<'a> fn(&'a Container) -> IndicatorFuture<'a>;

/// One indicator submitted to the link-time registry by `#[indicators]`.
pub struct HealthIndicator {
    /// `module_path!()` of the crate that declared it.
    pub origin: &'static str,
    /// The indicator's stable id (snake_case method name), its JSON key.
    pub name: &'static str,
    /// The probe this indicator participates in.
    pub kind: ProbeKind,
    /// `TypeId` of the owning provider, matched against the reachable set.
    pub provider_type_id: fn() -> TypeId,
    /// Resolves the owning provider and runs the check method.
    pub run: IndicatorRun,
}

::nest_rs_core::inventory::collect!(HealthIndicator);
