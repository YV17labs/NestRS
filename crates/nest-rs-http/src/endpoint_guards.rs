//! [`EndpointGuards`], the guards a module adds to the request of an in-band
//! endpoint, and what the transport reads of them.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, log_effective_chain, resolve_scoped,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, ContainerBuilder, Discoverable, Discovery, Posture, UnitView};
use nest_rs_guards::__private::{resolve_global_guards, split_at_covered, validate_guard_chain};
use nest_rs_guards::{Guard, GuardFor, GuardSpec, guard};

use crate::endpoint::{HttpEndpointMeta, InBand};
use crate::view::HttpView;

/// Guards a module adds to the request of the in-band endpoint whose units are
/// `V` — `/graphql` for `GraphqlView`, `/mcp` for `McpView` — composed with the
/// app's global guard pool and run once each, never instead of it. Listed as a
/// provider, so it is module-gated: a module the app does not import adds
/// nothing.
///
/// ```
/// use nest_rs_core::{Layer, UnitView, injectable, module};
/// use nest_rs_guards::{Denial, Guard, GuardFor};
/// use nest_rs_http::{EndpointGuards, HttpView, async_trait};
/// # struct ReportView;
/// # impl UnitView for ReportView {}
///
/// #[injectable]
/// #[derive(Default)]
/// struct ApiKeyGuard;
///
/// impl Layer for ApiKeyGuard {}
///
/// #[async_trait]
/// impl Guard for ApiKeyGuard {
///     async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
///         match HttpView::of(cx)?.request().header("x-api-key") {
///             Some(_) => Ok(()),
///             None => Err(Denial::unauthorized("an API key is required")),
///         }
///     }
/// }
///
/// impl GuardFor<HttpView> for ApiKeyGuard {}
///
/// #[module(providers = [ApiKeyGuard, EndpointGuards<ReportView, (ApiKeyGuard,)>])]
/// struct ReportsModule;
/// ```
///
/// Where the pool lists a guard the endpoint's guards name, they run at that
/// guard's place, the first one they name, so a pooled guard reading the
/// principal still runs after the authentication they carry; otherwise after
/// the pool. Declared by several modules, they run in import order, each once.
pub struct EndpointGuards<V: UnitView, G: GuardList>(PhantomData<fn() -> (V, G)>);

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

impl<V: UnitView, G: GuardList> Discoverable for EndpointGuards<V, G> {
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
/// code (`McpOptions::posture`).
pub struct EndpointPosture {
    endpoint: TypeId,
    posture: Posture,
}

impl EndpointPosture {
    /// `posture` for the endpoint whose units are `V`.
    pub fn of<V: UnitView>(posture: Posture) -> Self {
        Self {
            endpoint: TypeId::of::<V>(),
            posture,
        }
    }
}

/// What runs at a self-mount's request, before its surface sees it.
pub(crate) enum RequestStage {
    /// Nothing: a public surface, or one with no guard to run.
    Open,
    /// These guards, in the order they run.
    Guarded(Vec<ResolvedLayer<dyn Guard>>),
    /// A gated surface carrying in-band units with no guard at all: every
    /// request is refused.
    Denied,
}

/// Compose the guard stage at `meta`'s request, named `site` in boot errors:
/// the global pool, the surface's own guards, and the [`EndpointGuards`] the
/// app's modules declare for the in-band units it carries — or nothing for a
/// public surface, the app's opening ([`EndpointPosture`]) included.
pub(crate) fn request_stage(
    container: &Container,
    meta: &HttpEndpointMeta,
    site: &str,
) -> anyhow::Result<RequestStage> {
    let discovery = Discovery::new(container);
    let carries = meta.carries();
    let posture = match carries {
        Some(units) => declared_posture(&discovery, units, site)?,
        None => None,
    }
    .unwrap_or_else(|| meta.handler().posture());
    let declared: Vec<Arc<EndpointGuardsMeta>> = match carries {
        Some(units) => discovery
            .meta::<EndpointGuardsMeta>()
            .into_iter()
            .map(|found| found.meta)
            .filter(|declared| declared.endpoint == units.view)
            .collect(),
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
    let host = resolve_scoped(container, meta.guards(), LayerSite::Host, site)?;
    let pool = dedup_bucket(resolve_global_guards(container));
    let covers: Vec<TypeId> = endpoint.iter().map(|entry| entry.type_id).collect();
    let (head, tail) = split_at_covered(pool, &covers);
    let broadest: Vec<ResolvedLayer<dyn Guard>> =
        head.into_iter().chain(endpoint).chain(tail).collect();
    let chain = compose_chain::<dyn Guard>(broadest, host, Vec::new(), &[], site);
    validate_guard_chain(site, &chain).map_err(anyhow::Error::msg)?;
    log_effective_chain(site, "guards", &chain);
    Ok(match (chain.is_empty(), posture, carries) {
        (false, ..) => RequestStage::Guarded(chain),
        (true, Posture::Gated, Some(_)) => RequestStage::Denied,
        (true, ..) => RequestStage::Open,
    })
}

/// The posture the app declared for the endpoint carrying `units`, if it
/// declared one; two that disagree fail the boot.
fn declared_posture(
    discovery: &Discovery<'_>,
    units: InBand,
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
