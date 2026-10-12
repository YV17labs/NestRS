//! [`Site`], one dispatch site in a `static`: its handler, what its decorators
//! bound, and the [`UnitChains`] each container composes for it once.

use std::any::TypeId;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError, Weak};

use crate::container::__private::site_chains;
use crate::container::{Container, ContainerId};
use crate::dispatch::{ClassGate, GuardStage, GuardStageFactory};
use crate::error::UnresolvedLayerError;
use crate::exception_filter::__private::ErasedExceptionFilter;
use crate::exception_filter::{ExceptionFilterSpec, GlobalExceptionFilters};
use crate::handler::Handler;
use crate::interceptor::__private::InterceptorStage;
use crate::interceptor::{GlobalInterceptors, Interceptor, InterceptorSpec, module_interceptors};
use crate::layer::{Layer, LayerSite};
use crate::layer_chain::__private::{ResolvedLayer, compose_chain, resolve_global_layers};
use crate::layer_chain::LayerSpec;

/// One dispatch site — a route, a message, an operation, an endpoint — as its
/// decorators declared it, in a `static`: its [`Handler`] and its
/// [`SiteDeclaration`]. The [`UnitChains`] a container composes for it are the
/// container's; the site keeps only a `Weak` handle, so no `static` holds a
/// provider past its app.
pub struct Site {
    handler: &'static Handler,
    declared: SiteDeclaration,
    cache: SiteCache,
}

/// What a site's decorators bound beside its handler.
pub struct SiteDeclaration {
    /// The interceptors and exception filters bound on the host and the
    /// method, built once per container that composes the site.
    pub layers: fn() -> SiteLayers,
    /// Which app-wide layers the site folds in.
    pub global: GlobalLayers,
    /// Builds the site's guard stage against a container, `None` when the site
    /// runs no guard.
    pub guard_stage: Option<GuardStageFactory>,
    /// The posture's class gate.
    pub class_gate: Option<ClassGate>,
}

/// The interceptors and exception filters a site's decorators bound, each
/// list first-declared first.
#[derive(Default)]
pub struct SiteLayers {
    /// `#[use_interceptors]` on the host.
    pub host_interceptors: Vec<InterceptorSpec>,
    /// `#[use_interceptors]` beside the method.
    pub method_interceptors: Vec<InterceptorSpec>,
    /// `#[use_exception_filters]` on the host.
    pub host_exception_filters: Vec<ExceptionFilterSpec>,
    /// `#[use_exception_filters]` beside the method.
    pub method_exception_filters: Vec<ExceptionFilterSpec>,
}

/// Which app-wide layers a site folds in beside what its decorators bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalLayers {
    /// The unit stage: off for a unit nested in one that already ran it.
    pub unit_stage: bool,
    /// The exception filters the app lists.
    pub exception_filters: bool,
    /// The handler stage's app-wide interceptors: the app's, then the
    /// modules'.
    pub handler_stage: bool,
}

impl GlobalLayers {
    /// Every app-wide layer: a unit an edge accepts on its own.
    pub const ALL: Self = Self {
        unit_stage: true,
        exception_filters: true,
        handler_stage: true,
    };

    /// None: only what the site's decorators bound.
    pub const NONE: Self = Self {
        unit_stage: false,
        exception_filters: false,
        handler_stage: false,
    };
}

/// The [`Site`] of `handler`, declared in a `static`.
pub const fn declare_site(handler: &'static Handler, declared: SiteDeclaration) -> Site {
    Site {
        handler,
        declared,
        cache: SiteCache::new(),
    }
}

impl Site {
    /// The handler the site dispatches to.
    pub fn handler(&self) -> &'static Handler {
        self.handler
    }

    /// The chains `container` composed for this site, composing them on the
    /// first call. A layer that does not resolve refuses the site and nothing
    /// is kept: no unit runs without a layer its site declares.
    pub fn chains(&self, container: &Container) -> Result<Arc<UnitChains>, UnresolvedLayerError> {
        if let Some(chains) = self.cache.live_for(container.id()) {
            return Ok(chains);
        }
        self.compose_for(container)
    }

    /// The miss: this site's first unit, or another app in the process.
    #[cold]
    fn compose_for(&self, container: &Container) -> Result<Arc<UnitChains>, UnresolvedLayerError> {
        let mut extra = self.cache.extra();
        if let Some(chains) = self.cache.live_in(&extra, container.id()) {
            return Ok(chains);
        }
        let chains = Arc::new(UnitChains::compose(
            container,
            self.handler,
            &self.declared,
        )?);
        site_chains(container).hold(Arc::clone(&chains));
        let cached = Cached {
            container: container.id(),
            chains: Arc::downgrade(&chains),
        };
        if let Err(cached) = self.cache.primary.set(cached) {
            extra.retain(|held| held.chains.strong_count() > 0);
            extra.push(cached);
        }
        Ok(chains)
    }
}

/// A site's chains per container, held by `Weak` handles only.
struct SiteCache {
    /// The serving app's — the only one a real process fills.
    primary: OnceLock<Cached>,
    /// Further apps sharing the process (a suite boots several), allocated only
    /// when a second container reaches the site.
    extra: OnceLock<Mutex<Vec<Cached>>>,
}

/// Chains the cache reaches while their container lives, and never keeps
/// alive.
struct Cached {
    container: ContainerId,
    chains: Weak<UnitChains>,
}

impl Cached {
    fn live_for(&self, id: ContainerId) -> Option<Arc<UnitChains>> {
        (self.container == id)
            .then(|| self.chains.upgrade())
            .flatten()
    }
}

impl SiteCache {
    const fn new() -> Self {
        Self {
            primary: OnceLock::new(),
            extra: OnceLock::new(),
        }
    }

    fn live_for(&self, id: ContainerId) -> Option<Arc<UnitChains>> {
        self.primary.get().and_then(|cached| cached.live_for(id))
    }

    fn live_in(&self, extra: &[Cached], id: ContainerId) -> Option<Arc<UnitChains>> {
        self.live_for(id)
            .or_else(|| extra.iter().find_map(|cached| cached.live_for(id)))
    }

    /// The further apps' entries. The vector holds only `Weak` handles, so a
    /// poisoned lock is recovered: at worst a site composes twice.
    fn extra(&self) -> std::sync::MutexGuard<'_, Vec<Cached>> {
        self.extra
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// How a site's unit runs, chosen when its chains are composed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Shape {
    /// Nothing bound: the tail runs alone, nothing boxed.
    Bare,
    /// Guards or a class gate alone, run inline before the tail.
    GateOnly,
    /// Interceptors or exception filters: every stage.
    Layered,
}

/// The layers one site runs on one container, composed once: the unit stage,
/// the guard stage, the class gate, the exception filters and the handler
/// stage, each in the order it runs.
pub struct UnitChains {
    shape: Shape,
    pub(crate) unit_stage: Vec<ResolvedLayer<dyn Interceptor>>,
    pub(crate) guard_stage: Option<Arc<dyn GuardStage>>,
    pub(crate) class_gate: Option<ClassGate>,
    /// The method's first, then the host's, then the app's.
    pub(crate) exception_filters: Vec<ResolvedLayer<dyn ErasedExceptionFilter>>,
    /// Outermost first: the app's, the modules', the host's, the method's.
    pub(crate) handler_stage: Vec<ResolvedLayer<dyn Interceptor>>,
}

impl UnitChains {
    /// How the site's unit runs.
    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub(crate) fn compose(
        container: &Container,
        handler: &Handler,
        declared: &SiteDeclaration,
    ) -> Result<Self, UnresolvedLayerError> {
        let label = site_label(handler);
        let unit = handler.unit();
        let global = declared.global;
        let bound = (declared.layers)();

        let unit_stage = if global.unit_stage {
            let mut stage = resolve_all(
                container,
                module_interceptors(container, InterceptorStage::Unit),
                LayerSite::Global,
                &label,
            )?;
            stage.retain(|entry| entry.layer.serves(unit));
            // Stable: import order breaks a tie.
            stage.sort_by_key(|entry| entry.layer.priority());
            stage
        } else {
            Vec::new()
        };

        let mut global_interceptors = Vec::new();
        if global.handler_stage {
            global_interceptors.extend(resolve_global_layers::<GlobalInterceptors>(container));
            global_interceptors.extend(resolve_all(
                container,
                module_interceptors(container, InterceptorStage::Handler),
                LayerSite::Global,
                &label,
            )?);
            global_interceptors.retain(|entry| entry.layer.serves(unit));
        }
        let mut handler_stage = compose_chain(
            global_interceptors,
            resolve_all(
                container,
                bound.host_interceptors.iter().map(|spec| &spec.0),
                LayerSite::Host,
                &label,
            )?,
            resolve_all(
                container,
                bound.method_interceptors.iter().map(|spec| &spec.0),
                LayerSite::Method,
                &label,
            )?,
            &[],
            &label,
        );
        handler_stage.retain(|entry| {
            let at_the_unit_stage = unit_stage.iter().any(|ran| ran.type_id == entry.type_id);
            if at_the_unit_stage {
                report_unit_stage_duplicate(entry);
            }
            !at_the_unit_stage
        });

        let mut global_filters = Vec::new();
        if global.exception_filters {
            global_filters.extend(resolve_global_layers::<GlobalExceptionFilters>(container));
            global_filters.retain(|entry| entry.layer.serves(unit));
        }
        let mut exception_filters = compose_chain(
            global_filters,
            resolve_all(
                container,
                bound.host_exception_filters.iter().map(|spec| &spec.0),
                LayerSite::Host,
                &label,
            )?,
            resolve_all(
                container,
                bound.method_exception_filters.iter().map(|spec| &spec.0),
                LayerSite::Method,
                &label,
            )?,
            &[],
            &label,
        );
        // Closest first; stable, so each site keeps its own order.
        exception_filters.sort_by_key(|entry| match entry.source {
            LayerSite::Method => 0,
            LayerSite::Host => 1,
            LayerSite::Global => 2,
        });

        let guard_stage = match declared.guard_stage {
            Some(build) => build(container, handler)?,
            None => None,
        };
        let class_gate = declared.class_gate;

        let layered =
            !(unit_stage.is_empty() && exception_filters.is_empty() && handler_stage.is_empty());
        let shape = if layered {
            Shape::Layered
        } else if guard_stage.is_some() || class_gate.is_some() {
            Shape::GateOnly
        } else {
            Shape::Bare
        };
        let chains = Self {
            shape,
            unit_stage,
            guard_stage,
            class_gate,
            exception_filters,
            handler_stage,
        };
        chains.log_composed(&label);
        Ok(chains)
    }

    fn log_composed(&self, site: &str) {
        tracing::trace!(
            target: crate::target::LAYERS,
            site,
            shape = ?self.shape,
            unit_stage = joined_names(&self.unit_stage),
            guard_stage = self.guard_stage.is_some(),
            class_gate = self.class_gate.is_some(),
            exception_filters = joined_names(&self.exception_filters),
            handler_stage = joined_names(&self.handler_stage),
            "site composed",
        );
    }
}

/// A chain's layers as a log line names them, in the order they run.
fn joined_names<L: ?Sized>(chain: &[ResolvedLayer<L>]) -> String {
    chain
        .iter()
        .map(|entry| crate::type_name::shorten(entry.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The site as a boot error names it: the method on its host, or the address
/// of a framework endpoint.
fn site_label(handler: &Handler) -> String {
    if handler.method().is_empty() {
        format!("{} {}", handler.unit().name(), handler.address())
    } else {
        format!("{}::{}", handler.host_name(), handler.method())
    }
}

/// Every spec of `specs`, resolved as an entry from `source`; the first that
/// does not resolve refuses the site.
fn resolve_all<'s, L: ?Sized + 's>(
    container: &Container,
    specs: impl IntoIterator<Item = &'s LayerSpec<L>>,
    source: LayerSite,
    site: &str,
) -> Result<Vec<ResolvedLayer<L>>, UnresolvedLayerError> {
    specs
        .into_iter()
        .map(|spec| {
            let layer = spec
                .resolve(container)
                .ok_or_else(|| UnresolvedLayerError {
                    layer: spec.name,
                    site: site.to_owned(),
                })?;
            Ok(ResolvedLayer {
                type_id: spec.type_id,
                name: spec.name,
                source,
                layer,
            })
        })
        .collect()
}

/// Say once per process, at `debug`, that an interceptor listed at the handler
/// stage runs at the unit stage instead: it runs once, where its module put
/// it.
fn report_unit_stage_duplicate<L: Layer + ?Sized>(entry: &ResolvedLayer<L>) {
    static SEEN: LazyLock<Mutex<HashSet<(TypeId, LayerSite)>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    // A duplicate line is harmless; a swallowed one is not.
    let first_time = SEEN
        .lock()
        .map(|mut seen| seen.insert((entry.type_id, entry.source)))
        .unwrap_or(true);
    if first_time {
        tracing::debug!(
            target: crate::target::LAYERS,
            layer = crate::type_name::shorten(entry.name),
            kept = "unit stage",
            skipped = entry.source.label(),
            hint = "its module runs it at the unit stage, around the guards, for every unit it serves",
            "redundant layer declaration deduped",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::ContainerBuilder;
    use crate::exception_filter::exception_filter;
    use crate::interceptor::__private::register_interceptor;
    use crate::interceptor::interceptor;
    use crate::testing::{JOB, RUN, Recording, SEND};
    use crate::{Caught, ExceptionFilter, UnitContext, async_trait};

    struct First;
    struct Second;
    struct Third;
    struct Fourth;

    fn names<L: ?Sized>(chain: &[ResolvedLayer<L>]) -> Vec<String> {
        chain
            .iter()
            .map(|entry| crate::type_name::shorten(entry.name))
            .collect()
    }

    fn composed(
        container: &Container,
        handler: &Handler,
        declared: &SiteDeclaration,
    ) -> UnitChains {
        UnitChains::compose(container, handler, declared)
            .unwrap_or_else(|unresolved| panic!("the site composes: {unresolved}"))
    }

    fn declared(layers: fn() -> SiteLayers, global: GlobalLayers) -> SiteDeclaration {
        SiteDeclaration {
            layers,
            global,
            guard_stage: None,
            class_gate: None,
        }
    }

    fn app_lists(builder: ContainerBuilder, specs: Vec<InterceptorSpec>) -> ContainerBuilder {
        builder.provide(GlobalInterceptors(
            specs.into_iter().map(|spec| spec.0).collect(),
        ))
    }

    #[test]
    fn the_handler_stage_runs_the_apps_then_the_modules_then_the_hosts_then_the_methods() {
        let builder = ContainerBuilder::default()
            .provide(Recording::<First>::new("app"))
            .provide(Recording::<Second>::new("module"))
            .provide(Recording::<Third>::new("host"))
            .provide(Recording::<Fourth>::new("method"));
        let builder = register_interceptor::<Recording<Second>>(builder, InterceptorStage::Handler);
        let container = app_lists(builder, vec![interceptor::<Recording<First>>()]).build();
        let chains = composed(
            &container,
            &SEND,
            &declared(
                || SiteLayers {
                    host_interceptors: vec![interceptor::<Recording<Third>>()],
                    method_interceptors: vec![interceptor::<Recording<Fourth>>()],
                    ..SiteLayers::default()
                },
                GlobalLayers::ALL,
            ),
        );
        assert_eq!(
            names(&chains.handler_stage),
            [
                "Recording<First>",
                "Recording<Second>",
                "Recording<Third>",
                "Recording<Fourth>"
            ],
        );
    }

    #[test]
    fn a_priority_orders_within_the_apps_wide_bucket_and_import_order_breaks_a_tie() {
        let builder = ContainerBuilder::default()
            .provide(Recording::<First>::new("app"))
            .provide(Recording::<Second>::new("late module").with_priority(5))
            .provide(Recording::<Third>::new("early module").with_priority(-5));
        let builder = register_interceptor::<Recording<Second>>(builder, InterceptorStage::Handler);
        let builder = register_interceptor::<Recording<Third>>(builder, InterceptorStage::Handler);
        let container = app_lists(builder, vec![interceptor::<Recording<First>>()]).build();
        let chains = composed(
            &container,
            &SEND,
            &declared(SiteLayers::default, GlobalLayers::ALL),
        );
        assert_eq!(
            names(&chains.handler_stage),
            ["Recording<Third>", "Recording<First>", "Recording<Second>"],
        );
    }

    #[test]
    fn the_unit_stage_orders_by_priority_then_import_order() {
        let builder = ContainerBuilder::default()
            .provide(Recording::<First>::new("first"))
            .provide(Recording::<Second>::new("second").with_priority(-1))
            .provide(Recording::<Third>::new("third"));
        let builder = register_interceptor::<Recording<First>>(builder, InterceptorStage::Unit);
        let builder = register_interceptor::<Recording<Second>>(builder, InterceptorStage::Unit);
        let container =
            register_interceptor::<Recording<Third>>(builder, InterceptorStage::Unit).build();
        let chains = composed(
            &container,
            &SEND,
            &declared(SiteLayers::default, GlobalLayers::ALL),
        );
        assert_eq!(
            names(&chains.unit_stage),
            ["Recording<Second>", "Recording<First>", "Recording<Third>"],
        );
        assert!(chains.handler_stage.is_empty());
    }

    #[test]
    fn an_app_wide_layer_serving_another_edge_is_left_out_and_one_bound_at_the_site_runs() {
        let builder = ContainerBuilder::default()
            .provide(Recording::<First>::new("unit stage").serving(JOB))
            .provide(Recording::<Second>::new("app").serving(JOB))
            .provide(Recording::<Third>::new("bound").serving(JOB));
        let builder = register_interceptor::<Recording<First>>(builder, InterceptorStage::Unit);
        let container = app_lists(
            builder,
            vec![
                interceptor::<Recording<Second>>(),
                interceptor::<Recording<Third>>(),
            ],
        )
        .build();
        let layers = || SiteLayers {
            method_interceptors: vec![interceptor::<Recording<Third>>()],
            ..SiteLayers::default()
        };

        let message = composed(&container, &SEND, &declared(layers, GlobalLayers::ALL));
        assert!(
            message.unit_stage.is_empty(),
            "it serves jobs, not messages"
        );
        assert_eq!(
            names(&message.handler_stage),
            ["Recording<Third>"],
            "a layer a decorator binds runs where it is bound",
        );
        assert_eq!(message.handler_stage[0].source, LayerSite::Method);

        let job = composed(&container, &RUN, &declared(layers, GlobalLayers::ALL));
        assert_eq!(names(&job.unit_stage), ["Recording<First>"]);
        assert_eq!(
            names(&job.handler_stage),
            ["Recording<Second>", "Recording<Third>"],
        );
        assert_eq!(job.handler_stage[1].source, LayerSite::Global);
    }

    #[test]
    fn an_interceptor_at_the_unit_stage_runs_there_once_whatever_lists_it_at_the_handler_stage() {
        let logs = nest_rs_testing::LogCapture::install();
        let builder = ContainerBuilder::default().provide(Recording::<First>::new("timing"));
        let builder = register_interceptor::<Recording<First>>(builder, InterceptorStage::Unit);
        let container = app_lists(builder, vec![interceptor::<Recording<First>>()]).build();
        let chains = composed(
            &container,
            &SEND,
            &declared(
                || SiteLayers {
                    method_interceptors: vec![interceptor::<Recording<First>>()],
                    ..SiteLayers::default()
                },
                GlobalLayers::ALL,
            ),
        );
        assert_eq!(names(&chains.unit_stage), ["Recording<First>"]);
        assert!(
            chains.handler_stage.is_empty(),
            "it runs once, at the unit stage"
        );
        let said: Vec<_> = logs
            .find(crate::target::LAYERS, "redundant layer declaration deduped")
            .into_iter()
            .filter(|event| event.field("kept").as_deref() == Some("unit stage"))
            .collect();
        assert_eq!(said.len(), 1, "one line says where it runs: {said:#?}");
        assert_eq!(said[0].level, "debug");
        assert_eq!(
            said[0].field("skipped").as_deref(),
            Some("global"),
            "the app's listing, which already outranked the method's",
        );
    }

    #[test]
    fn a_module_interceptor_the_app_also_lists_runs_once_where_the_app_listed_it() {
        let builder = ContainerBuilder::default()
            .provide(Recording::<First>::new("first"))
            .provide(Recording::<Second>::new("second"));
        let builder = register_interceptor::<Recording<First>>(builder, InterceptorStage::Handler);
        let container = app_lists(
            builder,
            vec![
                interceptor::<Recording<Second>>(),
                interceptor::<Recording<First>>(),
            ],
        )
        .build();
        let chains = composed(
            &container,
            &SEND,
            &declared(SiteLayers::default, GlobalLayers::ALL),
        );
        assert_eq!(
            names(&chains.handler_stage),
            ["Recording<Second>", "Recording<First>"],
        );
    }

    /// Declines every failure; `M` gives each one a type of its own.
    struct Declines<M>(std::marker::PhantomData<fn() -> M>);
    impl<M: 'static> Layer for Declines<M> {}

    #[async_trait]
    impl<M: 'static> ExceptionFilter for Declines<M> {
        type Exception = dyn std::error::Error + Send + Sync;

        async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
            Caught::Declined
        }
    }

    fn declines<M>() -> Declines<M> {
        Declines(std::marker::PhantomData)
    }

    #[test]
    fn exception_filters_run_the_methods_first_then_the_hosts_then_the_apps() {
        let container = ContainerBuilder::default()
            .provide(declines::<First>())
            .provide(declines::<Second>())
            .provide(declines::<Third>())
            .provide(GlobalExceptionFilters(vec![
                exception_filter::<Declines<First>>().0,
            ]))
            .build();
        let chains = composed(
            &container,
            &SEND,
            &declared(
                || SiteLayers {
                    host_exception_filters: vec![exception_filter::<Declines<Second>>()],
                    method_exception_filters: vec![exception_filter::<Declines<Third>>()],
                    ..SiteLayers::default()
                },
                GlobalLayers::ALL,
            ),
        );
        assert_eq!(
            names(&chains.exception_filters),
            ["Declines<Third>", "Declines<Second>", "Declines<First>"],
        );
    }

    fn everything_app_wide() -> Container {
        let builder = ContainerBuilder::default()
            .provide(Recording::<First>::new("unit stage"))
            .provide(Recording::<Second>::new("app"))
            .provide(declines::<Third>());
        let builder = register_interceptor::<Recording<First>>(builder, InterceptorStage::Unit);
        app_lists(builder, vec![interceptor::<Recording<Second>>()])
            .provide(GlobalExceptionFilters(vec![
                exception_filter::<Declines<Third>>().0,
            ]))
            .build()
    }

    #[test]
    fn a_site_folds_only_the_app_wide_layers_it_declares() {
        let container = everything_app_wide();
        let nested = composed(
            &container,
            &SEND,
            &declared(
                SiteLayers::default,
                GlobalLayers {
                    unit_stage: false,
                    ..GlobalLayers::ALL
                },
            ),
        );
        assert!(nested.unit_stage.is_empty(), "the unit it nests in ran it");
        assert_eq!(names(&nested.handler_stage), ["Recording<Second>"]);
        assert_eq!(names(&nested.exception_filters), ["Declines<Third>"]);

        let endpoint = composed(
            &container,
            &SEND,
            &declared(
                SiteLayers::default,
                GlobalLayers {
                    handler_stage: false,
                    ..GlobalLayers::ALL
                },
            ),
        );
        assert_eq!(names(&endpoint.unit_stage), ["Recording<First>"]);
        assert!(endpoint.handler_stage.is_empty());
        assert_eq!(names(&endpoint.exception_filters), ["Declines<Third>"]);

        let declared_only = composed(
            &container,
            &SEND,
            &declared(SiteLayers::default, GlobalLayers::NONE),
        );
        assert_eq!(declared_only.shape(), Shape::Bare);
    }

    #[test]
    fn a_site_takes_the_shape_of_what_it_runs() {
        let container = Container::default();
        let bare = composed(
            &container,
            &SEND,
            &declared(SiteLayers::default, GlobalLayers::ALL),
        );
        assert_eq!(bare.shape(), Shape::Bare);

        let gated = composed(
            &container,
            &SEND,
            &SiteDeclaration {
                class_gate: Some(|_| Ok(())),
                ..declared(SiteLayers::default, GlobalLayers::ALL)
            },
        );
        assert_eq!(gated.shape(), Shape::GateOnly);

        let container = everything_app_wide();
        let layered = composed(
            &container,
            &SEND,
            &declared(SiteLayers::default, GlobalLayers::ALL),
        );
        assert_eq!(layered.shape(), Shape::Layered);
    }

    fn binds_an_unprovided_interceptor() -> SiteLayers {
        SiteLayers {
            method_interceptors: vec![interceptor::<Recording<Fourth>>()],
            ..SiteLayers::default()
        }
    }

    static UNRESOLVED: Site = declare_site(
        &SEND,
        SiteDeclaration {
            layers: binds_an_unprovided_interceptor,
            global: GlobalLayers::ALL,
            guard_stage: None,
            class_gate: None,
        },
    );

    #[test]
    fn a_layer_no_module_provides_refuses_the_site_naming_both_and_keeps_nothing() {
        let container = Container::default();
        for _ in 0..2 {
            let Err(refused) = UNRESOLVED.chains(&container) else {
                panic!("the site composed without a layer it declares");
            };
            assert!(
                refused
                    .layer
                    .ends_with("Recording<nest_rs_core::site::tests::Fourth>")
            );
            assert_eq!(refused.site, "ChatGateway::send");
        }

        let provided = ContainerBuilder::default()
            .provide(Recording::<Fourth>::new("now provided"))
            .build();
        let chains = UNRESOLVED
            .chains(&provided)
            .unwrap_or_else(|refused| panic!("a container providing it composes: {refused}"));
        assert_eq!(names(&chains.handler_stage), ["Recording<Fourth>"]);
    }

    fn binds_one_interceptor() -> SiteLayers {
        SiteLayers {
            method_interceptors: vec![interceptor::<Recording<First>>()],
            ..SiteLayers::default()
        }
    }

    static CACHED: Site = declare_site(
        &SEND,
        SiteDeclaration {
            layers: binds_one_interceptor,
            global: GlobalLayers::ALL,
            guard_stage: None,
            class_gate: None,
        },
    );

    fn providing_one() -> Container {
        ContainerBuilder::default()
            .provide(Recording::<First>::new("cached"))
            .build()
    }

    #[test]
    fn a_site_composes_once_per_container_and_holds_no_chain_of_its_own() {
        let first = providing_one();
        let composed = CACHED.chains(&first).expect("composes");
        let again = CACHED.chains(&first).expect("composes");
        assert!(
            Arc::ptr_eq(&composed, &again),
            "composed once for a container"
        );

        let second = providing_one();
        let other = CACHED.chains(&second).expect("composes");
        assert!(
            !Arc::ptr_eq(&composed, &other),
            "each container composes its own"
        );

        let held = Arc::downgrade(&composed);
        drop((composed, again, first));
        assert!(
            held.upgrade().is_none(),
            "the chains went with their container: the site keeps only a weak handle",
        );
        assert!(
            CACHED
                .chains(&second)
                .is_ok_and(|chains| Arc::ptr_eq(&chains, &other))
        );
    }
}
