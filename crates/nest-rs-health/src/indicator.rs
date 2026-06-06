//! Health indicator contract and link-time registry.
//!
//! An application opts in by tagging methods on an `#[injectable]` provider's
//! impl block with `#[indicators]` (orchestrator) plus per-method
//! `#[liveness]` / `#[readiness]` / `#[startup]`. Each tagged method submits
//! one [`HealthIndicator`] entry to a link-time `inventory` registry.
//!
//! [`crate::HealthService`] drains the registry at probe time and filters by
//! [`ReachableProviders`](::nest_rs_core::ReachableProviders) — an indicator
//! whose provider is not in the app's module tree is silently skipped, the
//! same module-gating as the rest of the discovery system.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;

use nest_rs_core::Container;
use serde::Serialize;

/// Which Kubernetes-style probe an indicator participates in. A method's
/// `#[liveness]` / `#[readiness]` / `#[startup]` attribute maps one-to-one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeKind {
    Liveness,
    Readiness,
    Startup,
}

/// `up` when the indicator's check returned `Ok`; `down` otherwise. Serialized
/// lowercase so the JSON body matches the Kubernetes/Terminus vocabulary
/// operators already know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IndicatorStatus {
    Up,
    Down,
}

/// Outcome of a single indicator check, included in a [`ProbeReport`].
#[derive(Clone, Debug, Serialize)]
pub struct IndicatorReport {
    pub name: &'static str,
    pub status: IndicatorStatus,
    /// `Some` only when the check failed — the stringified `anyhow` error so
    /// the JSON body carries enough for an operator to triage without a log
    /// dive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregated outcome of a probe: an overall `status` plus per-indicator
/// reports. The HTTP status maps from `status` — `200` for `up`, `503` for
/// `down`.
#[derive(Clone, Debug, Serialize)]
pub struct ProbeReport {
    pub status: IndicatorStatus,
    /// Up indicators, keyed by name (Terminus-style: easy to grep in JSON
    /// logs without iterating a list).
    pub info: std::collections::BTreeMap<&'static str, IndicatorReport>,
    /// Down indicators, keyed by name. Empty when `status == Up`.
    pub error: std::collections::BTreeMap<&'static str, IndicatorReport>,
    /// Every indicator that ran, up or down — the union of `info` and `error`.
    pub details: std::collections::BTreeMap<&'static str, IndicatorReport>,
}

impl ProbeReport {
    /// Empty up — no indicators ran for this probe (the framework default
    /// before any app registers one).
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

pub type IndicatorFuture<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

pub type IndicatorRun = for<'a> fn(&'a Container) -> IndicatorFuture<'a>;

/// One indicator submitted to the link-time registry by `#[indicators]`. The
/// `run` thunk resolves the owning provider from the container and invokes
/// the method.
pub struct HealthIndicator {
    /// `"<method_name>"` — the indicator's stable id (snake_case method
    /// name), used as the JSON key and the structured-log field.
    pub name: &'static str,
    pub kind: ProbeKind,
    /// `TypeId::of::<Provider>()` — checked against
    /// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so an
    /// unreachable provider's indicators do not run.
    pub provider_type_id: fn() -> TypeId,
    pub run: IndicatorRun,
}

::nest_rs_core::inventory::collect!(HealthIndicator);
