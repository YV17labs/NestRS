//! Health indicator contract: the link-time registry `#[indicators]` fills,
//! and the container metadata the module opening a resource attaches.

use std::any::TypeId;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use nest_rs_core::{Container, Module};
use nest_rs_http::__private::{DocumentedAnswer, DocumentedPayload};
use nest_rs_http::schemars::JsonSchema;
use nest_rs_http::{HeaderValue, IntoResponse, Response, StatusCode};
use serde::Serialize;

/// Which Kubernetes-style probe an indicator participates in.
///
/// Exhaustive on purpose: Kubernetes' container probes are these three, so a
/// caller matches each and never needs a `_` arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
#[expect(
    clippy::exhaustive_enums,
    reason = "closed by Kubernetes, whose container probes are these three"
)]
pub enum ProbeKind {
    /// Liveness — is the process healthy enough to keep running?
    Liveness,
    /// Readiness — can the process serve traffic right now?
    Readiness,
    /// Startup — has the process finished initializing?
    Startup,
}

impl ProbeKind {
    /// The word a line names this probe by.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Liveness => "liveness",
            Self::Readiness => "readiness",
            Self::Startup => "startup",
        }
    }
}

/// `up` when the indicator's check returned `Ok`; `down` otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(crate = "nest_rs_http::schemars")]
#[non_exhaustive]
pub enum IndicatorStatus {
    /// The check returned `Ok`.
    Up,
    /// The check returned an error.
    Down,
}

/// Outcome of a single indicator check, included in a [`ProbeReport`].
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(crate = "nest_rs_http::schemars")]
#[non_exhaustive]
pub struct IndicatorReport {
    /// The indicator's stable id: its snake_case method name, or the crate
    /// owning a resource and its instance (`redis:cache`).
    pub name: Cow<'static, str>,
    /// Whether this indicator's check passed.
    pub status: IndicatorStatus,
    /// `Some` only when the check failed — a **fixed, opaque** reason
    /// (`"check failed"`, for an error or a panic / `"timed out"` / `"probe
    /// deadline exceeded"`), never the indicator's own error, which is logged
    /// on `nest_rs::health` — at `warn`, a panic at `error`: `/health/*` is
    /// routinely unauthenticated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregated outcome of a probe: an overall `status` (`200` up, `503` down)
/// plus per-indicator reports.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[schemars(crate = "nest_rs_http::schemars")]
#[non_exhaustive]
pub struct ProbeReport {
    /// The overall probe result — `down` if any indicator is down.
    pub status: IndicatorStatus,
    /// Up indicators, keyed by name.
    pub info: std::collections::BTreeMap<Cow<'static, str>, IndicatorReport>,
    /// Down indicators, keyed by name. Empty when `status == Up`.
    pub error: std::collections::BTreeMap<Cow<'static, str>, IndicatorReport>,
    /// Every indicator that ran, up or down — the union of `info` and `error`.
    pub details: std::collections::BTreeMap<Cow<'static, str>, IndicatorReport>,
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
                    info.insert(r.name.clone(), r.clone());
                }
                IndicatorStatus::Down => {
                    error.insert(r.name.clone(), r.clone());
                    status = IndicatorStatus::Down;
                }
            }
            details.insert(r.name.clone(), r);
        }
        Self {
            status,
            info,
            error,
            details,
        }
    }
}

/// A route returning it answers `200` up and `503` down. The type states the
/// `200`, and a route names the `503`'s body with
/// `#[api(error(503 = ProbeReport))]`.
impl DocumentedAnswer for ProbeReport {}

impl DocumentedPayload for ProbeReport {
    type Payload = Self;
}

impl IntoResponse for ProbeReport {
    fn into_response(self) -> Response {
        let status = match self.status {
            IndicatorStatus::Up => StatusCode::OK,
            IndicatorStatus::Down => StatusCode::SERVICE_UNAVAILABLE,
        };
        // Never an empty 200 on failure: an orchestrator would read it as healthy.
        match serde_json::to_vec(&self) {
            Ok(body) => Response::builder()
                .status(status)
                .content_type(HeaderValue::from_static("application/json"))
                .body(body),
            Err(error) => {
                tracing::error!(
                    target: crate::TARGET,
                    %error,
                    "health report failed to serialize",
                );
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .finish()
            }
        }
    }
}

/// The boxed future one indicator check resolves to, borrowing the [`Container`].
pub type IndicatorFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// The check an indicator runs: a function pointer, because `inventory`
/// submits a [`HealthIndicator`] from a `const` context.
pub type IndicatorRun = for<'a> fn(&'a Container) -> IndicatorFuture<'a>;

/// One indicator submitted to the link-time registry by `#[indicators]`.
pub struct HealthIndicator {
    origin: &'static str,
    name: &'static str,
    kind: ProbeKind,
    host_type_id: fn() -> TypeId,
    run: IndicatorRun,
}

impl HealthIndicator {
    /// `module_path!()` of the crate that declared it.
    pub const fn origin(&self) -> &'static str {
        self.origin
    }

    /// The indicator's stable id (snake_case method name), its JSON key.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The probe this indicator participates in.
    pub const fn kind(&self) -> ProbeKind {
        self.kind
    }

    /// `TypeId` of the owning provider, which a reader gates on
    /// [`ReachableProviders`](::nest_rs_core::ReachableProviders) as the probe
    /// does.
    pub fn host_type_id(&self) -> TypeId {
        (self.host_type_id)()
    }

    /// Resolves the owning provider and runs the check method.
    pub(crate) const fn run(&self) -> IndicatorRun {
        self.run
    }
}

/// The entry `#[indicators]` submits for one method; a hand-built entry is a
/// test's, never an app's.
pub const fn health_indicator(
    origin: &'static str,
    name: &'static str,
    kind: ProbeKind,
    host_type_id: fn() -> TypeId,
    run: IndicatorRun,
) -> HealthIndicator {
    HealthIndicator {
        origin,
        name,
        kind,
        host_type_id,
        run,
    }
}

::nest_rs_core::inventory::collect!(HealthIndicator);

/// An indicator a module attaches to the container from its `register` — how a
/// resource the framework opens, once per instance, ships its check: the
/// module opening the resource attaches it, so no app imports a check of its
/// own; an `inventory` entry cannot be generic, and metadata is gated by
/// construction, since only an imported module registers.
///
/// Its name is the probe body's key, read off the crate owning the resource
/// and the instance, never chosen: `seaorm`, `redis`, `redis:cache`,
/// `storage` — a package's rooted at its crate, `nestrs_acme`. An app's own
/// indicator keeps its method name through [`indicators`](crate::indicators).
///
/// ```
/// use nest_rs_core::{App, Container, ContainerBuilder, Module, Registering, module};
/// use nest_rs_health::{HealthModule, HealthService, IndicatorEntry, IndicatorFuture, ProbeKind};
///
/// fn ping(_container: &Container) -> IndicatorFuture<'_> {
///     Box::pin(async { Ok(()) })
/// }
///
/// struct PoolModule;
///
/// impl Module for PoolModule {
///     fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
///         builder.provide_meta(IndicatorEntry::new::<Self>("pool", ProbeKind::Readiness, ping))
///     }
/// }
///
/// #[module(imports = [HealthModule, PoolModule])]
/// struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let app = App::builder().module::<AppModule>().build().await?;
/// # let health = app.container().get::<HealthService>().expect("HealthModule provides it");
/// let ready = health.probe(ProbeKind::Readiness).await;
/// assert!(ready.details.contains_key("pool"));
/// # Ok(())
/// # }
/// ```
pub struct IndicatorEntry {
    pub(crate) name: Cow<'static, str>,
    pub(crate) kind: ProbeKind,
    /// The module that attached it, as a boot refusal names it.
    pub(crate) origin: &'static str,
    pub(crate) run: IndicatorRun,
}

impl IndicatorEntry {
    /// The indicator `name` on the `kind` probe, attached by the module `M`
    /// and answering through `run`.
    pub fn new<M: Module>(
        name: impl Into<Cow<'static, str>>,
        kind: ProbeKind,
        run: IndicatorRun,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            origin: std::any::type_name::<M>(),
            run,
        }
    }
}
