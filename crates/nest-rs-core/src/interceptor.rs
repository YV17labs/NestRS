//! [`Interceptor`], the layer around one unit on any edge, [`Next`], the rest
//! of the unit it runs around, and the [`InterceptorSpec`] an app lists one
//! with.

use std::any::{TypeId, type_name};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::access::ProviderOrder;
use crate::ambient::install_pending;
use crate::container::Container;
use crate::dispatch::{ErasedTail, after_unit_stage};
use crate::layer::Layer;
use crate::layer_chain::__private::ResolvedLayer;
use crate::layer_chain::{GlobalSpecs, LayerSpec};
use crate::outcome::Outcome;
use crate::site::UnitChains;
use crate::unit_context::{UnitContext, take_pending};

/// Around one unit on any edge: before the rest of it, after it, or instead of
/// it — answering through the edge's view when that view can hold a reply.
///
/// An interceptor runs at the handler stage, after the guards and the
/// posture's class gate, unless its module declares it at the unit stage
/// (`#[interceptor(stage = unit)]`), around the guards. What it returns says
/// whether the unit ran or was answered ([`Settled`](crate::Settled)), so
/// returning neither does not compile; and once the rest of the unit failed,
/// answering success leaves it failed, with one `warn` naming the
/// interceptor.
///
/// ```
/// use std::error::Error;
/// use std::sync::Arc;
/// use nest_rs_core::{
///     Caught, ExceptionFilter, Interceptor, Layer, Next, Outcome, UnitContext, async_trait,
/// };
///
/// struct Audit;
/// impl Layer for Audit {}
///
/// #[async_trait]
/// impl Interceptor for Audit {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         let outcome = next.run(cx).await;
///         nest_rs_core::tracing::info!(
///             target: "app::audit",
///             unit = cx.unit().name(),
///             ok = outcome.is_ok(),
///             "unit audited",
///         );
///         outcome
///     }
/// }
///
/// struct Opaque;
/// impl Layer for Opaque {}
///
/// #[async_trait]
/// impl ExceptionFilter for Opaque {
///     type Exception = dyn Error + Send + Sync;
///
///     async fn catch(&self, _: &Self::Exception, _: &mut UnitContext<'_>) -> Caught {
///         Caught::Declined
///     }
/// }
/// # use std::any::TypeId;
/// # use nest_rs_core::__private::{
/// #     GlobalLayers, HandlerDeclaration, MetaLevels, Site, SiteDeclaration, SiteLayers, Tail,
/// #     declare_handler, declare_site, declare_unit, dispatch, new_unit_context,
/// # };
/// # use nest_rs_core::{Handler, Posture, UnitView, exception_filter, interceptor, operation_log::Kind};
/// # struct Chat;
/// # struct ChatView;
/// # impl UnitView for ChatView {}
/// # struct Reply;
/// # impl Tail for Reply {
/// #     async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
/// #         Err(std::io::Error::other("the store is down").into())
/// #     }
/// # }
/// # static SEND: Handler = declare_handler(HandlerDeclaration {
/// #     unit: declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message"),
/// #     host: TypeId::of::<Chat>(),
/// #     host_name: "Chat",
/// #     method: "send",
/// #     address: "send",
/// #     posture: Posture::Gated,
/// #     meta: MetaLevels::default,
/// # });
/// # fn layers() -> SiteLayers {
/// #     SiteLayers {
/// #         method_interceptors: vec![interceptor::<Audit>(), interceptor::<Arc<Audit>>(), interceptor::<Arc<Arc<Audit>>>()],
/// #         method_exception_filters: vec![exception_filter::<Opaque>()],
/// #         ..SiteLayers::default()
/// #     }
/// # }
/// # static SITE: Site = declare_site(&SEND, SiteDeclaration {
/// #     layers,
/// #     global: GlobalLayers::ALL,
/// #     guard_stage: None,
/// #     class_gate: None,
/// # });
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let container = nest_rs_core::Container::builder()
/// #     .provide(Audit)
/// #     .provide(Arc::new(Audit))
/// #     .provide(Arc::new(Arc::new(Audit)))
/// #     .provide(Opaque)
/// #     .build()?;
/// # let chains = SITE.chains(&container)?;
/// # let unit = tokio::spawn(async move {
/// #     let mut view = ChatView;
/// #     let mut cx = new_unit_context(&SEND, &mut view, None, None);
/// #     dispatch(&mut cx, &chains, &mut Reply).await.is_err()
/// # });
/// # assert!(unit.await?);
/// # Ok(())
/// # }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an interceptor",
    label = "bound as an interceptor here",
    note = "implement `Interceptor` for it — `async fn intercept(&self, cx: &mut \
            UnitContext<'_>, next: Next<'_>) -> Outcome` — beside `Layer`"
)]
#[async_trait]
pub trait Interceptor: Layer {
    /// Run around one unit: call `next.run(cx)` once to run the rest of it,
    /// or answer instead of it.
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome;
}

// Hand-written to match what `#[async_trait]` generates: the macro would box
// the already boxed future a second time on every call.
impl<T: Interceptor + ?Sized> Interceptor for Arc<T> {
    fn intercept<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        cx: &'life1 mut UnitContext<'life2>,
        next: Next<'life3>,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        (**self).intercept(cx, next)
    }
}

/// The rest of the unit an [`Interceptor`] runs around: the interceptors still
/// to run, then the stages after theirs, then the handler.
///
/// It runs once — [`run`](Self::run) consumes it:
///
/// ```compile_fail,E0382
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait};
///
/// struct Retry;
/// impl Layer for Retry {}
///
/// #[async_trait]
/// impl Interceptor for Retry {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         match next.run(cx).await {
///             Ok(settled) => Ok(settled),
///             Err(_) => next.run(cx).await,
///         }
///     }
/// }
/// ```
pub struct Next<'a> {
    links: &'a [ResolvedLayer<dyn Interceptor>],
    rest: Rest<'a>,
}

/// What follows the interceptors of a [`Next`].
pub(crate) enum Rest<'a> {
    /// The unit stage's: the guard stage, the class gate, the exception
    /// filters, the handler stage and the tail.
    Unit {
        chains: &'a UnitChains,
        tail: &'a mut (dyn ErasedTail + 'a),
    },
    /// The handler stage's: the tail.
    Tail(&'a mut (dyn ErasedTail + 'a)),
}

impl<'a> Next<'a> {
    pub(crate) fn new(links: &'a [ResolvedLayer<dyn Interceptor>], rest: Rest<'a>) -> Self {
        Self { links, rest }
    }

    /// Run the rest of the unit, once. What the stage running it attached
    /// becomes ambient inside it, and only there.
    pub async fn run(self, cx: &mut UnitContext<'_>) -> Outcome {
        let pending = take_pending(cx);
        let outcome = if pending.is_empty() {
            self.proceed(cx).await
        } else {
            install_pending(pending, self.proceed(cx)).await
        };
        cx.set_failed_inner(outcome.as_ref().err());
        outcome
    }

    async fn proceed(self, cx: &mut UnitContext<'_>) -> Outcome {
        match self.links.split_first() {
            Some((link, links)) => call_link(link, cx, Next::new(links, self.rest)).await,
            None => match self.rest {
                Rest::Unit { chains, tail } => after_unit_stage(cx, chains, tail).await,
                Rest::Tail(tail) => tail.call_boxed(cx).await,
            },
        }
    }
}

/// Run the handler stage: `links` outermost first, then the tail.
///
/// Not `Next::proceed`: the unit stage's future holds this one, so holding
/// that one here would make it infinitely sized (E0733).
pub(crate) async fn run_handler_stage(
    cx: &mut UnitContext<'_>,
    links: &[ResolvedLayer<dyn Interceptor>],
    tail: &mut (dyn ErasedTail + '_),
) -> Outcome {
    match links.split_first() {
        Some((link, links)) => call_link(link, cx, Next::new(links, Rest::Tail(tail))).await,
        None => tail.call_boxed(cx).await,
    }
}

/// Run one interceptor, keeping the unit failed when it answers success after
/// the rest of the unit failed.
async fn call_link(
    link: &ResolvedLayer<dyn Interceptor>,
    cx: &mut UnitContext<'_>,
    next: Next<'_>,
) -> Outcome {
    cx.set_failed_inner(None);
    // `as_ref()` dispatches on the erased interceptor: the `Arc<T>` forward
    // would nest a second boxed future around every link.
    let outcome = link.layer.as_ref().intercept(cx, next).await;
    match (outcome, cx.take_failed_inner()) {
        (Ok(settled), Some(inner)) => {
            tracing::warn!(
                target: crate::target::LAYERS,
                layer = crate::type_name::shorten(link.name),
                unit = cx.unit().name(),
                error = crate::error_message(inner.error()),
                "a layer answered success for a failed unit; the unit stays failed",
            );
            Err(inner.answered(settled))
        }
        (outcome, _) => outcome,
    }
}

/// One interceptor an app lists, resolved against the container when a site
/// composes its chains.
pub struct InterceptorSpec(pub(crate) LayerSpec<dyn Interceptor>);

impl fmt::Debug for InterceptorSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("InterceptorSpec")
            .field(&self.0.name)
            .finish()
    }
}

/// The [`InterceptorSpec`] of `I`, the provider of that type a module imports.
pub fn interceptor<I: Interceptor>() -> InterceptorSpec {
    InterceptorSpec(layer_spec::<I>())
}

fn layer_spec<I: Interceptor>() -> LayerSpec<dyn Interceptor> {
    LayerSpec::new(TypeId::of::<I>(), type_name::<I>(), |container| {
        container
            .get::<I>()
            .map(|interceptor| interceptor as Arc<dyn Interceptor>)
    })
}

/// The interceptors an app lists for every unit, at the handler stage.
pub(crate) struct GlobalInterceptors(pub(crate) Vec<LayerSpec<dyn Interceptor>>);

impl GlobalSpecs for GlobalInterceptors {
    type Layer = dyn Interceptor;

    fn specs(&self) -> &[LayerSpec<dyn Interceptor>] {
        &self.0
    }
}

/// `interceptor` is private: its tier-2 items live here, reached only through
/// the crate's `__private`.
pub(crate) mod __private {
    use super::{Interceptor, layer_spec};
    use crate::container::ContainerBuilder;
    use crate::layer_chain::LayerSpec;

    /// Where `#[interceptor]` runs one: around the guards, or after them.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum InterceptorStage {
        /// `#[interceptor(stage = unit)]`: around the guard stage.
        Unit,
        /// `#[interceptor]`: after the guards and the class gate, with the
        /// interceptors the app lists.
        Handler,
    }

    impl InterceptorStage {
        /// The stage as `#[interceptor]` spells it.
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::Unit => "unit",
                Self::Handler => "handler",
            }
        }
    }

    /// An interceptor a module brings for every unit it serves, attached to
    /// its provider so only an imported module's reaches a site.
    pub struct InterceptorRegistration {
        /// Its stage.
        pub stage: InterceptorStage,
        pub(crate) spec: LayerSpec<dyn Interceptor>,
    }

    /// Register `I`, already provided, as an interceptor at `stage` — what
    /// `#[interceptor]` expands to.
    pub fn register_interceptor<I: Interceptor>(
        builder: ContainerBuilder,
        stage: InterceptorStage,
    ) -> ContainerBuilder {
        let spec = layer_spec::<I>();
        tracing::debug!(
            target: crate::target::LAYERS,
            layer = crate::type_name::shorten(spec.name),
            stage = stage.as_str(),
            "interceptor registered",
        );
        builder.attach_meta::<I, InterceptorRegistration>(InterceptorRegistration { stage, spec })
    }
}

/// The interceptors a module brought for `stage`, in import order.
pub(crate) fn module_interceptors(
    container: &Container,
    stage: __private::InterceptorStage,
) -> Vec<&LayerSpec<dyn Interceptor>> {
    let order = container.get::<ProviderOrder>();
    let mut found: Vec<&LayerSpec<dyn Interceptor>> = container
        .metadata_entries(TypeId::of::<__private::InterceptorRegistration>())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let registration = entry
                .meta
                .downcast_ref::<__private::InterceptorRegistration>()?;
            (registration.stage == stage).then_some(&registration.spec)
        })
        .collect();
    // Stable: registration order where the container records no declaration
    // order.
    found.sort_by_key(|spec| order.as_ref().map_or(0, |order| order.rank(spec.type_id)));
    found
}
