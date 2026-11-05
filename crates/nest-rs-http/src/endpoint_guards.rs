//! [`EndpointGuards`], the guards a module adds to the request of an in-band
//! endpoint, and what the transport reads of them.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, log_effective_chain, resolve_scoped,
};
use nest_rs_core::LayerSite;
use nest_rs_core::{Container, ContainerBuilder, Discoverable, Discovery, Posture, UnitView};
use nest_rs_guards::__private::{
    GuardSpec, guard, resolve_global_guards, split_at_covered, validate_guard_chain,
};
use nest_rs_guards::{Guard, GuardFor};

use crate::endpoint::{Carried, HttpEndpointMeta};
use crate::view::HttpView;

/// A view whose units an HTTP endpoint's request carries — `GraphqlView` on
/// `/graphql`, `McpView` on `/mcp` — and so the view an [`EndpointGuards`]
/// names that endpoint by. A route and a gateway's messages are no such units:
/// their guards are their own `#[use_guards]`.
#[diagnostic::on_unimplemented(
    message = "no HTTP endpoint's request carries `{Self}` units",
    label = "`EndpointGuards` names an endpoint by the view of the units its request carries",
    note = "`GraphqlView` names `/graphql` and `McpView` names `/mcp`; a route and a gateway's \
            upgrade take their guards from `#[use_guards]`"
)]
pub trait CarriedView: UnitView {}

/// Guards a module adds to the request of the endpoint whose units are `V` —
/// `/graphql` for `GraphqlView`, `/mcp` for `McpView` — composed with the
/// app's global guard pool and run once each, never instead of it. Listed as a
/// provider, so it is module-gated: a module the app does not import adds
/// nothing, and a declaration whose endpoint the app does not serve fails the
/// boot. `McpView`'s example declares it for `/mcp`.
///
/// Where the pool lists a guard the endpoint's guards name, they run at that
/// guard's place, the first one they name, so a pooled guard reading the
/// principal still runs after the authentication they carry; otherwise after
/// the pool. Declared by several modules, they run in import order, each once.
pub struct EndpointGuards<V: CarriedView, G: GuardList>(PhantomData<fn() -> (V, G)>);

/// A tuple of guards that check an HTTP request — `(AuthnGuard,)` to eight of
/// them — run in the order written. Sealed: only tuples are lists.
pub trait GuardList: sealed::Sealed + 'static {
    /// Each guard's spec, in order.
    #[doc(hidden)]
    fn __specs() -> Vec<GuardSpec>;
}

mod sealed {
    pub trait Sealed {}
}

macro_rules! guard_list {
    ($($guard:ident),+) => {
        impl<$($guard: GuardFor<HttpView> + 'static),+> sealed::Sealed for ($($guard,)+) {}

        impl<$($guard: GuardFor<HttpView> + 'static),+> GuardList for ($($guard,)+) {
            fn __specs() -> Vec<GuardSpec> {
                vec![$(guard::<$guard>()),+]
            }
        }
    };
}

guard_list!(A);
guard_list!(A, B);
guard_list!(A, B, C);
guard_list!(A, B, C, D);
guard_list!(A, B, C, D, E);
guard_list!(A, B, C, D, E, F);
guard_list!(A, B, C, D, E, F, G);
guard_list!(A, B, C, D, E, F, G, H);

impl<V: CarriedView, G: GuardList> Discoverable for EndpointGuards<V, G> {
    fn injected() -> Vec<TypeId> {
        G::__specs().iter().map(|spec| spec.type_id).collect()
    }

    fn injected_names() -> Vec<&'static str> {
        G::__specs().iter().map(|spec| spec.name).collect()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Self, EndpointGuardsMeta>(EndpointGuardsMeta {
            endpoint: TypeId::of::<V>(),
            declared_by: std::any::type_name::<Self>(),
            specs: G::__specs(),
        })
    }
}

/// What one [`EndpointGuards`] declares: the guards it adds to the request of
/// the endpoint whose units are `endpoint`.
pub struct EndpointGuardsMeta {
    endpoint: TypeId,
    declared_by: &'static str,
    specs: Vec<GuardSpec>,
}

impl EndpointGuardsMeta {
    /// The view of the units the endpoint carries.
    pub fn endpoint(&self) -> TypeId {
        self.endpoint
    }

    /// The declaration, as a boot error names it.
    pub fn declared_by(&self) -> &'static str {
        self.declared_by
    }

    /// The guards, in the order declared.
    pub fn specs(&self) -> &[GuardSpec] {
        &self.specs
    }
}

/// The posture an app declares for the in-band endpoint whose units are
/// `endpoint`, over the one its `Handler` states: an opening the app wrote in
/// code (`McpAccess::Public`).
pub struct EndpointPosture {
    endpoint: TypeId,
    posture: Posture,
}

impl EndpointPosture {
    /// `posture` for the endpoint whose units are `V`.
    pub fn of<V: CarriedView>(posture: Posture) -> Self {
        Self {
            endpoint: TypeId::of::<V>(),
            posture,
        }
    }
}

/// What an app writes for a surface no guard runs at: one sentence, whichever
/// surface it is and whether the boot refuses it or files a line.
pub const NO_GUARD_REMEDY: &str = "list `GlobalGuards<(...)>` in the providers of the module the \
     app boots, bind `#[use_guards(...)]` on the host, `EndpointGuards<View, (...)>` on an \
     endpoint carrying units, or open it on purpose: `#[public]` on a route, a message or a \
     GraphQL operation, `McpModule::for_root(McpAccess::Public)` on an MCP endpoint";

/// What a surface no guard runs at does with the units its requests carry, as
/// the `no guard at the request` line says it.
pub(crate) mod no_guard_then {
    /// A gated surface: nothing admits a request.
    pub(crate) const DENIES_EVERY_UNIT: &str = "denies every unit";
    /// An in-band surface: only a unit open on purpose reaches a caller.
    pub(crate) const RUNS_PUBLIC_UNITS_ONLY: &str = "runs #[public] units only";
}

/// What runs at a self-mount's request, before its surface sees it.
pub(crate) enum RequestStage {
    /// Nothing: a public surface, or one with no guard to run.
    Open,
    /// These guards, in the order they run.
    Guarded(Vec<ResolvedLayer<dyn Guard>>),
    /// A gated surface carrying units, with no guard at all: every request is
    /// refused.
    Denied,
    /// A surface deciding each unit it carries, with no guard at all: every
    /// request is let through, and a unit it carries decides alone.
    InBand,
}

/// Compose the guard stage at `meta`'s request, named `site` in boot errors:
/// the global pool, the surface's own guards, and the [`EndpointGuards`] the
/// app's modules declare for the units it carries — or nothing for a
/// public surface, the app's opening ([`EndpointPosture`]) included.
pub(crate) fn request_stage(
    container: &Container,
    meta: &HttpEndpointMeta,
    site: &str,
) -> anyhow::Result<RequestStage> {
    let discovery = Discovery::new(container);
    let carried = meta.carried();
    let posture = match carried {
        Some(units) => declared_posture(&discovery, units, site)?,
        None => None,
    }
    .unwrap_or_else(|| meta.handler().posture());
    let declared: Vec<Arc<EndpointGuardsMeta>> = match carried {
        Some(units) => declared_for(&discovery, units.view),
        None => Vec::new(),
    };
    if posture == Posture::Public {
        if let Some(declared) = declared.first() {
            anyhow::bail!(
                "{site} is declared public, and `{}` declares guards for its request: the opening \
                 would drop them — keep the endpoint gated, or remove the declaration",
                declared.declared_by,
            );
        }
        return Ok(RequestStage::Open);
    }

    let chain = composed_chain(container, &declared, meta.guards(), site)?;
    validate_guard_chain(site, &chain).map_err(anyhow::Error::msg)?;
    log_effective_chain(site, "guards", &chain);
    Ok(match (chain.is_empty(), posture, carried) {
        (false, ..) => RequestStage::Guarded(chain),
        (true, Posture::Gated, Some(_)) => RequestStage::Denied,
        (true, Posture::InBand, Some(_)) => RequestStage::InBand,
        (true, ..) => RequestStage::Open,
    })
}

/// The guards a gated request carrying `V` units runs, in the order they run:
/// the global pool and the [`EndpointGuards`] the app's modules declare for
/// them, named `site` in a boot error.
pub fn carried_request_chain<V: CarriedView>(
    container: &Container,
    site: &str,
) -> anyhow::Result<Vec<ResolvedLayer<dyn Guard>>> {
    let declared = declared_for(&Discovery::new(container), TypeId::of::<V>());
    composed_chain(container, &declared, &[], site)
}

/// What the app's modules declare for the endpoint whose units are `endpoint`.
fn declared_for(discovery: &Discovery<'_>, endpoint: TypeId) -> Vec<Arc<EndpointGuardsMeta>> {
    discovery
        .meta::<EndpointGuardsMeta>()
        .into_iter()
        .map(|found| found.meta)
        .filter(|declared| declared.endpoint == endpoint)
        .collect()
}

/// The pool, `declared`'s guards and the surface's own `host` guards, each once
/// and in the order they run.
fn composed_chain(
    container: &Container,
    declared: &[Arc<EndpointGuardsMeta>],
    host: &[GuardSpec],
    site: &str,
) -> anyhow::Result<Vec<ResolvedLayer<dyn Guard>>> {
    let mut endpoint_specs: Vec<&GuardSpec> = Vec::new();
    for spec in declared.iter().flat_map(|declared| declared.specs()) {
        if !endpoint_specs
            .iter()
            .any(|seen| seen.type_id == spec.type_id)
        {
            endpoint_specs.push(spec);
        }
    }
    let endpoint = resolve_scoped(container, endpoint_specs, LayerSite::Host, site)?;
    let host = resolve_scoped(container, host, LayerSite::Host, site)?;
    let pool = dedup_bucket(resolve_global_guards(container));
    let covers: Vec<TypeId> = endpoint.iter().map(|entry| entry.type_id).collect();
    let (head, tail) = split_at_covered(pool, &covers);
    let broadest: Vec<ResolvedLayer<dyn Guard>> =
        head.into_iter().chain(endpoint).chain(tail).collect();
    Ok(compose_chain::<dyn Guard>(
        broadest,
        host,
        Vec::new(),
        &[],
        site,
    ))
}

/// Refuse every [`EndpointGuards`] whose units no mounted endpoint's request
/// carries — `carried`, the views the self-mounts declared: its guards would
/// run nowhere while the module reads as protected.
pub(crate) fn check_declarations_reached(
    container: &Container,
    carried: &[TypeId],
) -> anyhow::Result<()> {
    let unreached: Vec<&'static str> = Discovery::new(container)
        .meta::<EndpointGuardsMeta>()
        .into_iter()
        .filter(|found| !carried.contains(&found.meta.endpoint))
        .map(|found| found.meta.declared_by)
        .collect();
    if unreached.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "guards declared for an endpoint the app does not mount would run nowhere: {} — import \
         the module that serves that endpoint, or remove the declaration",
        unreached.join(", "),
    )
}

/// The posture the app declared for the endpoint carrying `units`, if it
/// declared one; two that disagree fail the boot.
fn declared_posture(
    discovery: &Discovery<'_>,
    units: Carried,
    site: &str,
) -> anyhow::Result<Option<Posture>> {
    let mut declared = discovery
        .meta::<EndpointPosture>()
        .into_iter()
        .map(|found| found.meta)
        .filter(|declared| declared.endpoint == units.view)
        .map(|declared| declared.posture);
    let Some(first) = declared.next() else {
        return Ok(None);
    };
    if declared.any(|other| other != first) {
        anyhow::bail!("{site} is declared both {first:?} and otherwise — declare its posture once",);
    }
    Ok(Some(first))
}
