//! [`dispatch`], the one function every edge runs a unit through, and the
//! seams it calls: the edge's [`Tail`], the [`GuardStage`] and the
//! [`ClassGate`].
//!
//! Outermost first: the unit stage, the guard stage, the class gate, what the
//! guards attached made ambient, the exception filters, the handler stage, the
//! tail (`.claude/decisions/one-execution-pipeline.md`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use crate::ambient::install_pending;
use crate::container::Container;
use crate::error::UnresolvedLayerError;
use crate::exception_filter::catch;
use crate::handler::Handler;
use crate::interceptor::{Next, Rest, run_handler_stage};
use crate::outcome::{Failure, Outcome};
use crate::site::{Shape, UnitChains};
use crate::unit_context::{UnitContext, take_pending};

/// A future the pipeline holds behind a box.
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What an edge runs last: its pipes, the handler, the posture's mask and the
/// reply written into its view.
pub trait Tail: Send {
    /// Run the handler for the unit `cx` describes.
    fn call(&mut self, cx: &mut UnitContext<'_>) -> impl Future<Output = Outcome> + Send;
}

/// A [`Tail`] behind a box, as the handler stage's [`Next`] holds it.
pub(crate) trait ErasedTail: Send {
    fn call_boxed<'a>(&'a mut self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Outcome>;
}

impl<T: Tail> ErasedTail for T {
    fn call_boxed<'a>(&'a mut self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Outcome> {
        Box::pin(self.call(cx))
    }
}

/// The guards of a site, run as one stage before its class gate. What they
/// attach is ambient only once the whole stage and the gate passed; a denial
/// comes back answered, and no exception filter sees it.
pub trait GuardStage: Send + Sync {
    /// Run every guard of the stage on the unit `cx` describes.
    fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>>;
}

/// Builds a site's guard stage against a container; `None` when no guard
/// applies there.
pub type GuardStageFactory =
    fn(&Container, &Handler) -> Result<Option<Arc<dyn GuardStage>>, UnresolvedLayerError>;

/// The posture's class gate: synchronous, since it reads what the guards
/// attached, so a gated unit pays no box for it.
pub type ClassGate = for<'b> fn(&mut UnitContext<'b>) -> Result<(), Failure>;

/// Run one unit of a site through its `chains`, the rest of the unit being
/// `tail`.
///
/// A `Bare` site runs the tail alone and allocates nothing; a `GateOnly` one
/// runs the guard stage and the gate inline before it; a `Layered` one runs
/// every stage, holding the tail behind one box.
pub fn dispatch<'a, 'u, T: Tail>(
    cx: &'a mut UnitContext<'u>,
    chains: &'a UnitChains,
    tail: &'a mut T,
) -> impl Future<Output = Outcome> + Send {
    match chains.shape() {
        Shape::Bare => Dispatch::Bare {
            tail: tail.call(cx),
        },
        Shape::GateOnly => Dispatch::Gating {
            gates: gates_passed(cx, chains, tail),
            then: gated::<T>,
        },
        Shape::Layered => Dispatch::Layered {
            stages: Next::new(&chains.unit_stage, Rest::Unit { chains, tail }).run(cx),
        },
    }
}

pin_project_lite::pin_project! {
    /// [`dispatch`]'s future, one variant per shape.
    ///
    /// Not an `async fn`: a debug build reserves every arm's copy of the tail
    /// in one poll frame, which stays on the stack while the tail runs. A
    /// `GateOnly` site builds its tail in a frame of its own once the gates
    /// passed, as the ambient context's `Install` does.
    #[project = DispatchProj]
    #[project_replace = DispatchOwned]
    enum Dispatch<F, G, C, I, L> {
        Bare { #[pin] tail: F },
        Gating { #[pin] gates: G, then: fn(C) -> I },
        Gated { #[pin] tail: I },
        Layered { #[pin] stages: L },
        Building,
    }
}

impl<F, G, C, I, L> Dispatch<F, G, C, I, L> {
    /// Build the tail of a `GateOnly` site from what its gates handed back, in
    /// a frame of its own: the copies of the tail this makes are off the stack
    /// before it is polled.
    #[inline(never)]
    fn start(mut self: Pin<&mut Self>, passed: C) {
        if let DispatchOwned::Gating { then, .. } = self.as_mut().project_replace(Self::Building) {
            self.set(Self::Gated { tail: then(passed) });
        }
    }
}

impl<F, G, C, I, L> Future for Dispatch<F, G, C, I, L>
where
    F: Future<Output = Outcome>,
    G: Future<Output = Result<C, Failure>>,
    I: Future<Output = Outcome>,
    L: Future<Output = Outcome>,
{
    type Output = Outcome;

    fn poll(mut self: Pin<&mut Self>, task: &mut Context<'_>) -> Poll<Outcome> {
        if let DispatchProj::Gating { gates, .. } = self.as_mut().project() {
            match ready!(gates.poll(task)) {
                Ok(passed) => self.as_mut().start(passed),
                Err(refused) => return Poll::Ready(Err(refused)),
            }
        }
        match self.project() {
            DispatchProj::Bare { tail } => tail.poll(task),
            DispatchProj::Gated { tail } => tail.poll(task),
            DispatchProj::Layered { stages } => stages.poll(task),
            DispatchProj::Gating { .. } | DispatchProj::Building => {
                unreachable!("a dispatch is polled again after building its tail panicked")
            }
        }
    }
}

/// A `GateOnly` site's guard stage and class gate, handing back what its tail
/// is built from once they passed.
async fn gates_passed<'a, 'u, T>(
    cx: &'a mut UnitContext<'u>,
    chains: &UnitChains,
    tail: &'a mut T,
) -> Result<(&'a mut UnitContext<'u>, &'a mut T), Failure> {
    pass_gates(cx, chains).await?;
    Ok((cx, tail))
}

/// A `GateOnly` site's tail, with what its guards attached made ambient.
fn gated<'a, 'u, T: Tail>(
    (cx, tail): (&'a mut UnitContext<'u>, &'a mut T),
) -> impl Future<Output = Outcome> {
    install_pending(take_pending(cx), tail.call(cx))
}

/// The guard stage, then the class gate. A refused unit keeps nothing they
/// attached.
async fn pass_gates(cx: &mut UnitContext<'_>, chains: &UnitChains) -> Result<(), Failure> {
    let passed = match &chains.guard_stage {
        Some(stage) => stage.check(cx).await,
        None => Ok(()),
    }
    .and_then(|()| match chains.class_gate {
        Some(gate) => gate(cx),
        None => Ok(()),
    });
    if passed.is_err() {
        drop(take_pending(cx));
    }
    passed
}

/// Everything inside the unit stage: the guards and the gate, then, with what
/// they attached made ambient, the exception filters around the handler stage.
pub(crate) async fn after_unit_stage(
    cx: &mut UnitContext<'_>,
    chains: &UnitChains,
    tail: &mut (dyn ErasedTail + '_),
) -> Outcome {
    pass_gates(cx, chains).await?;
    install_pending(take_pending(cx), filtered(cx, chains, tail)).await
}

/// The handler stage, its failure handed to the exception filters.
async fn filtered(
    cx: &mut UnitContext<'_>,
    chains: &UnitChains,
    tail: &mut (dyn ErasedTail + '_),
) -> Outcome {
    match run_handler_stage(cx, &chains.handler_stage, tail).await {
        Ok(settled) => Ok(settled),
        Err(failure) => catch(cx, &chains.exception_filters, failure).await,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ambient::Pending;
    use crate::container::ContainerBuilder;
    use crate::exception_filter::GlobalExceptionFilters;
    use crate::interceptor::__private::{InterceptorStage, register_interceptor};
    use crate::interceptor::GlobalInterceptors;
    use crate::outcome::__private::settled;
    use crate::site::{GlobalLayers, SiteDeclaration, SiteLayers};
    use crate::testing::{
        HandlerFailed, Recording, RecordingGuards, RecordingTail, Refused, SEND, TrailView, record,
        recording_gate, run_counting,
    };
    use crate::unit_context::new_unit_context;
    use crate::{
        AmbientValue, Caught, ExceptionFilter, Interceptor, Layer, Next, async_trait,
        exception_filter, interceptor,
    };

    struct UnitStage;
    struct AppListed;
    struct ModuleBrought;
    struct OnTheHost;
    struct OnTheMethod;

    fn admitting(
        _: &Container,
        _: &Handler,
    ) -> Result<Option<Arc<dyn GuardStage>>, UnresolvedLayerError> {
        Ok(Some(Arc::new(RecordingGuards { admit: true })))
    }

    fn denying(
        _: &Container,
        _: &Handler,
    ) -> Result<Option<Arc<dyn GuardStage>>, UnresolvedLayerError> {
        Ok(Some(Arc::new(RecordingGuards { admit: false })))
    }

    /// Declines every failure, writing that it was tried.
    struct Declining;
    impl Layer for Declining {}

    #[async_trait]
    impl ExceptionFilter for Declining {
        type Exception = dyn std::error::Error + Send + Sync;

        async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
            record(cx, "filter");
            Caught::Declined
        }
    }

    /// Every stage of a site: a unit-stage interceptor a module brings, guards
    /// and a gate, a filter, and the handler stage's four buckets.
    fn every_stage() -> Container {
        let builder = ContainerBuilder::default()
            .provide(Recording::<UnitStage>::new("unit"))
            .provide(Recording::<AppListed>::new("app"))
            .provide(Recording::<ModuleBrought>::new("module"))
            .provide(Recording::<OnTheHost>::new("host"))
            .provide(Recording::<OnTheMethod>::new("method"))
            .provide(Declining)
            .provide(GlobalInterceptors(vec![
                interceptor::<Recording<AppListed>>().0,
            ]));
        let builder = register_interceptor::<Recording<UnitStage>>(builder, InterceptorStage::Unit);
        register_interceptor::<Recording<ModuleBrought>>(builder, InterceptorStage::Handler)
            .build()
            .expect("the container builds")
    }

    fn bound_on_the_host_and_method() -> SiteLayers {
        SiteLayers {
            host_interceptors: vec![interceptor::<Recording<OnTheHost>>()],
            method_interceptors: vec![interceptor::<Recording<OnTheMethod>>()],
            method_exception_filters: vec![exception_filter::<Declining>()],
            ..SiteLayers::default()
        }
    }

    fn composed(container: &Container, declared: &SiteDeclaration) -> UnitChains {
        UnitChains::compose(container, &SEND, declared)
            .unwrap_or_else(|unresolved| panic!("the site composes: {unresolved}"))
    }

    async fn run<T: Tail>(chains: &UnitChains, tail: &mut T) -> (Outcome, Vec<String>) {
        let (outcome, steps, _) = run_then_read(chains, tail).await;
        (outcome, steps)
    }

    /// Run one unit: what it came to, what ran, and what the edge reads
    /// ambient once `dispatch` returned.
    async fn run_then_read<T: Tail>(
        chains: &UnitChains,
        tail: &mut T,
    ) -> (Outcome, Vec<String>, Option<Arc<Attached>>) {
        let mut view = TrailView::new();
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        let outcome = dispatch(&mut cx, chains, tail).await;
        let read = cx.ambient::<Attached>();
        (outcome, view.steps(), read)
    }

    /// A site whose guards and gate are all it runs.
    fn gate_only(guard_stage: GuardStageFactory, class_gate: Option<ClassGate>) -> UnitChains {
        let chains = composed(
            &Container::default(),
            &SiteDeclaration {
                layers: SiteLayers::default,
                global: GlobalLayers::ALL,
                guard_stage: Some(guard_stage),
                class_gate,
            },
        );
        assert_eq!(chains.shape(), Shape::GateOnly);
        chains
    }

    #[tokio::test]
    async fn the_stages_run_in_order_and_unwind_in_reverse() {
        let container = every_stage();
        let chains = composed(
            &container,
            &SiteDeclaration {
                layers: bound_on_the_host_and_method,
                global: GlobalLayers::ALL,
                guard_stage: Some(admitting),
                class_gate: Some(recording_gate),
            },
        );
        assert_eq!(chains.shape(), Shape::Layered);

        let (outcome, steps) = run(&chains, &mut RecordingTail::ok()).await;
        assert!(outcome.is_ok());
        assert_eq!(
            steps,
            [
                "unit in",
                "guards",
                "gate",
                "app in",
                "module in",
                "host in",
                "method in",
                "handler",
                "method out",
                "host out",
                "module out",
                "app out",
                "unit out",
            ],
        );

        let (outcome, steps) = run(&chains, &mut RecordingTail::failing()).await;
        assert!(outcome.is_err_and(|failure| failure.find::<HandlerFailed>().is_some()));
        assert_eq!(
            steps,
            [
                "unit in",
                "guards",
                "gate",
                "app in",
                "module in",
                "host in",
                "method in",
                "handler",
                "method out",
                "host out",
                "module out",
                "app out",
                "filter",
                "unit out",
            ],
            "the filters answer the handler stage's failure, inside the unit stage",
        );
    }

    #[tokio::test]
    async fn a_guard_stage_denial_reaches_the_unit_stage_and_no_filter() {
        let container = every_stage();
        let chains = composed(
            &container,
            &SiteDeclaration {
                layers: bound_on_the_host_and_method,
                global: GlobalLayers::ALL,
                guard_stage: Some(denying),
                class_gate: Some(recording_gate),
            },
        );

        let (outcome, steps) = run(&chains, &mut RecordingTail::ok()).await;
        assert_eq!(steps, ["unit in", "guards", "unit out"]);
        let failure = outcome.expect_err("the unit was denied");
        assert!(failure.is_answered(), "the guard stage answered it");
        assert!(failure.find::<Refused>().is_some());
    }

    /// Answers success whatever the rest of the unit came to.
    struct Swallowing;
    impl Layer for Swallowing {}

    #[async_trait]
    impl Interceptor for Swallowing {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            drop(next.run(cx).await);
            Ok(settled())
        }
    }

    /// Writes what the rest of the unit came to.
    struct Observing;
    impl Layer for Observing {}

    #[async_trait]
    impl Interceptor for Observing {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            let outcome = next.run(cx).await;
            record(
                cx,
                if outcome.is_ok() {
                    "observed ok"
                } else {
                    "observed failure"
                },
            );
            outcome
        }
    }

    fn observing_then_swallowing() -> SiteLayers {
        SiteLayers {
            method_interceptors: vec![interceptor::<Observing>(), interceptor::<Swallowing>()],
            ..SiteLayers::default()
        }
    }

    #[tokio::test]
    async fn an_ok_after_a_failed_inner_unit_stays_failed() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = ContainerBuilder::default()
            .provide(Observing)
            .provide(Swallowing)
            .build()
            .expect("the container builds");
        let chains = composed(
            &container,
            &SiteDeclaration {
                layers: observing_then_swallowing,
                global: GlobalLayers::ALL,
                guard_stage: None,
                class_gate: None,
            },
        );

        let (outcome, steps) = run(&chains, &mut RecordingTail::failing()).await;
        assert_eq!(
            steps,
            ["handler", "observed failure"],
            "the layer outside the one that answered success sees the failure",
        );
        let failure = outcome.expect_err("the unit stays failed");
        assert!(failure.is_answered(), "the layer's reply stands");
        assert!(
            failure.find::<HandlerFailed>().is_some(),
            "the original error is kept"
        );
        let warned = logs.expect_one(
            crate::target::LAYERS,
            "a layer answered success for a failed unit; the unit stays failed",
        );
        assert_eq!(warned.level, "warn");
        assert_eq!(warned.field("layer").as_deref(), Some("Swallowing"));
        assert_eq!(warned.field("error").as_deref(), Some("the handler failed"));
    }

    #[derive(Debug, PartialEq)]
    struct Attached(&'static str);
    impl AmbientValue for Attached {}

    /// Attaches before running the rest of the unit, and writes what is ambient
    /// once it ran.
    struct Attaching;
    impl Layer for Attaching {}

    #[async_trait]
    impl Interceptor for Attaching {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            cx.attach(Attached("by the interceptor"));
            let outcome = next.run(cx).await;
            record(
                cx,
                format!("after next: {:?}", crate::ambient::<Attached>()),
            );
            outcome
        }
    }

    /// Writes what is ambient where the handler runs.
    struct Reading;

    impl Tail for Reading {
        async fn call(&mut self, cx: &mut UnitContext<'_>) -> Outcome {
            record(
                cx,
                format!("in the handler: {:?}", crate::ambient::<Attached>()),
            );
            Ok(settled())
        }
    }

    fn attaching() -> SiteLayers {
        SiteLayers {
            method_interceptors: vec![interceptor::<Attaching>()],
            ..SiteLayers::default()
        }
    }

    #[tokio::test]
    async fn pending_attachments_are_visible_inside_next_only() {
        let container = ContainerBuilder::default()
            .provide(Attaching)
            .build()
            .expect("the container builds");
        let chains = composed(
            &container,
            &SiteDeclaration {
                layers: attaching,
                global: GlobalLayers::ALL,
                guard_stage: None,
                class_gate: None,
            },
        );
        let (outcome, steps) = run(&chains, &mut Reading).await;
        assert!(outcome.is_ok());
        assert_eq!(
            steps,
            [
                "in the handler: Some(Attached(\"by the interceptor\"))",
                "after next: None",
            ],
        );
    }

    /// A guard stage attaching `Attached("by a guard")`, then admitting or
    /// denying.
    struct AttachingGuards(bool);

    impl GuardStage for AttachingGuards {
        fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>> {
            Box::pin(async move {
                cx.attach(Attached("by a guard"));
                if self.0 {
                    Ok(())
                } else {
                    Err(Failure::from(Refused))
                }
            })
        }
    }

    fn attaching_and_admitting(
        _: &Container,
        _: &Handler,
    ) -> Result<Option<Arc<dyn GuardStage>>, UnresolvedLayerError> {
        Ok(Some(Arc::new(AttachingGuards(true))))
    }

    fn attaching_and_denying(
        _: &Container,
        _: &Handler,
    ) -> Result<Option<Arc<dyn GuardStage>>, UnresolvedLayerError> {
        Ok(Some(Arc::new(AttachingGuards(false))))
    }

    /// The class gate, writing `gate refused`, attaching `Attached("by the
    /// gate")` and refusing.
    fn refusing_gate(cx: &mut UnitContext<'_>) -> Result<(), Failure> {
        record(cx, "gate refused");
        cx.attach(Attached("by the gate"));
        Err(Failure::from(Refused))
    }

    /// Writes what the unit carries once the rest of it ran: attached and
    /// still pending, or ambient.
    struct Outside;
    impl Layer for Outside {}

    #[async_trait]
    impl Interceptor for Outside {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            let outcome = next.run(cx).await;
            record(cx, format!("after next: {:?}", cx.ambient::<Attached>()));
            outcome
        }
    }

    fn at_the_unit_stage(
        guard_stage: GuardStageFactory,
        class_gate: Option<ClassGate>,
    ) -> UnitChains {
        let builder = ContainerBuilder::default().provide(Outside);
        let container = register_interceptor::<Outside>(builder, InterceptorStage::Unit)
            .build()
            .expect("the container builds");
        composed(
            &container,
            &SiteDeclaration {
                layers: SiteLayers::default,
                global: GlobalLayers::ALL,
                guard_stage: Some(guard_stage),
                class_gate,
            },
        )
    }

    #[tokio::test]
    async fn what_the_guards_attach_is_ambient_past_them_and_never_outside_or_when_denied() {
        let refused: Option<ClassGate> = Some(refusing_gate);
        for (guard_stage, class_gate, admitted, expected) in [
            (
                attaching_and_admitting as GuardStageFactory,
                None,
                true,
                vec![
                    "in the handler: Some(Attached(\"by a guard\"))",
                    "after next: None",
                ],
            ),
            (attaching_and_denying, None, false, vec!["after next: None"]),
            (
                attaching_and_admitting,
                refused,
                false,
                vec!["gate refused", "after next: None"],
            ),
        ] {
            let chains = at_the_unit_stage(guard_stage, class_gate);
            let (outcome, steps, read) = run_then_read(&chains, &mut Reading).await;
            assert_eq!(outcome.is_ok(), admitted);
            assert_eq!(steps, expected);
            assert!(
                read.is_none(),
                "the edge reads nothing past the unit: {expected:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_gate_only_site_makes_what_its_guards_attach_ambient_in_its_tail_alone() {
        let refused: Option<ClassGate> = Some(refusing_gate);
        for (guard_stage, class_gate, admitted, expected) in [
            (
                attaching_and_admitting as GuardStageFactory,
                None,
                true,
                vec!["in the handler: Some(Attached(\"by a guard\"))"],
            ),
            (attaching_and_denying, None, false, vec![]),
            (
                attaching_and_admitting,
                refused,
                false,
                vec!["gate refused"],
            ),
        ] {
            let (outcome, steps, read) =
                run_then_read(&gate_only(guard_stage, class_gate), &mut Reading).await;
            assert_eq!(outcome.is_ok(), admitted);
            assert_eq!(steps, expected);
            assert!(
                read.is_none(),
                "the edge reads nothing past the unit: {expected:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_gate_only_site_denied_by_its_guards_runs_no_tail_and_answers_the_denial() {
        let (outcome, steps) = run(
            &gate_only(denying, Some(recording_gate)),
            &mut RecordingTail::ok(),
        )
        .await;
        assert_eq!(steps, ["guards"], "no gate, no handler");
        let failure = outcome.expect_err("the unit was denied");
        assert!(failure.is_answered(), "the guard stage answered it");
        assert!(failure.find::<Refused>().is_some());
    }

    #[tokio::test]
    async fn a_class_gate_refusal_reaches_the_unit_stage_and_runs_nothing_after_it() {
        let container = every_stage();
        let layered = composed(
            &container,
            &SiteDeclaration {
                layers: bound_on_the_host_and_method,
                global: GlobalLayers::ALL,
                guard_stage: Some(admitting),
                class_gate: Some(refusing_gate),
            },
        );
        assert_eq!(layered.shape(), Shape::Layered);
        let gated = gate_only(admitting, Some(refusing_gate));
        for (chains, expected) in [
            (
                &layered,
                vec!["unit in", "guards", "gate refused", "unit out"],
            ),
            (&gated, vec!["guards", "gate refused"]),
        ] {
            let (outcome, steps, read) = run_then_read(chains, &mut RecordingTail::failing()).await;
            assert_eq!(
                steps,
                expected,
                "no handler-stage interceptor, no filter, no handler on a {:?} site",
                chains.shape(),
            );
            assert!(
                outcome.is_err_and(|failure| failure.find::<Refused>().is_some()),
                "the gate's refusal is what the unit came to",
            );
            assert!(read.is_none(), "a refused unit installs nothing");
        }
    }

    /// Counts its calls, allocating nothing.
    #[derive(Default)]
    struct Counted(usize);

    impl Tail for Counted {
        async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
            self.0 += 1;
            Ok(settled())
        }
    }

    fn bare() -> UnitChains {
        let chains = composed(
            &Container::default(),
            &SiteDeclaration {
                layers: SiteLayers::default,
                global: GlobalLayers::ALL,
                guard_stage: None,
                class_gate: None,
            },
        );
        assert_eq!(chains.shape(), Shape::Bare);
        chains
    }

    #[test]
    fn a_bare_site_runs_its_tail_alone_and_allocates_nothing() {
        let chains = bare();
        let mut view = TrailView::new();
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        let mut tail = Counted::default();
        let (outcome, allocations) = run_counting(dispatch(&mut cx, &chains, &mut tail));
        assert!(outcome.is_ok());
        assert_eq!(tail.0, 1, "the tail ran once");
        assert_eq!(allocations, 0, "no stage, no box, no install");
    }

    #[tokio::test]
    async fn a_gate_only_site_runs_its_guards_and_gate_inline_before_the_tail() {
        let (outcome, steps) = run(
            &gate_only(admitting, Some(recording_gate)),
            &mut RecordingTail::ok(),
        )
        .await;
        assert!(outcome.is_ok());
        assert_eq!(steps, ["guards", "gate", "handler"]);
    }

    /// Bytes a tail holds across an await: enough that a copy of it shows.
    const HELD: usize = 32 * 1024;

    /// What the pipeline may add around the tail, its own state and frames,
    /// never a second copy of the tail.
    const DISPATCH_OVERHEAD: usize = 1024;

    /// Where the stack stands in the frame that calls it.
    #[inline(never)]
    fn stack_mark() -> usize {
        let mark = 0u8;
        std::hint::black_box(&mark) as *const u8 as usize
    }

    /// A tail holding `N` bytes across an await, marking where the stack stood
    /// once it resumed.
    #[derive(Default)]
    struct Heavy<const N: usize> {
        resumed_at: usize,
    }

    impl<const N: usize> Tail for Heavy<N> {
        async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
            let held = [0u8; N];
            tokio::task::yield_now().await;
            std::hint::black_box(&held);
            self.resumed_at = stack_mark();
            Ok(settled())
        }
    }

    /// How much deeper a [`Heavy`] tail resumes when `chains` dispatch it than
    /// when it is awaited alone, from the same frame.
    async fn deeper_than_alone<const N: usize>(chains: &UnitChains) -> usize {
        let mut view = TrailView::new();
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        let mut tail = Heavy::<N>::default();
        assert!(tail.call(&mut cx).await.is_ok());
        let alone = tail.resumed_at;
        assert!(dispatch(&mut cx, chains, &mut tail).await.is_ok());
        alone.saturating_sub(tail.resumed_at)
    }

    #[tokio::test]
    async fn a_bare_dispatch_resumes_its_tail_no_deeper_than_awaited_alone() {
        let deeper = deeper_than_alone::<HELD>(&bare()).await;
        assert!(
            deeper < DISPATCH_OVERHEAD,
            "a bare dispatch resumes a {HELD}-byte tail {deeper} bytes deeper than alone",
        );
    }

    #[tokio::test]
    async fn a_gate_only_dispatch_resumes_its_tail_no_deeper_than_awaited_alone() {
        let deeper = deeper_than_alone::<HELD>(&gate_only(admitting, Some(recording_gate))).await;
        assert!(
            deeper < DISPATCH_OVERHEAD,
            "a gate-only dispatch resumes a {HELD}-byte tail {deeper} bytes deeper than alone",
        );

        let mut view = TrailView::new();
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        let mut tail = Heavy::<HELD>::default();
        let mut attached = Pending::default();
        attached.attach(Arc::new(Attached("by a guard")));
        assert!(install_pending(attached, tail.call(&mut cx)).await.is_ok());
        let installed = tail.resumed_at;
        let chains = gate_only(attaching_and_admitting, Some(recording_gate));
        assert!(dispatch(&mut cx, &chains, &mut tail).await.is_ok());
        let deeper = installed.saturating_sub(tail.resumed_at);
        assert!(
            deeper < DISPATCH_OVERHEAD,
            "a gate-only dispatch resumes a {HELD}-byte tail {deeper} bytes deeper than the \
             install of what its guards attached",
        );
    }

    #[tokio::test]
    async fn a_layered_dispatch_reaches_as_deep_whatever_its_tail_holds() {
        let container = ContainerBuilder::default()
            .provide(Declining)
            .provide(GlobalExceptionFilters(vec![
                exception_filter::<Declining>().0,
            ]))
            .build()
            .expect("the container builds");
        let chains = composed(
            &container,
            &SiteDeclaration {
                layers: SiteLayers::default,
                global: GlobalLayers::ALL,
                guard_stage: None,
                class_gate: None,
            },
        );
        assert_eq!(chains.shape(), Shape::Layered);
        let held = deeper_than_alone::<HELD>(&chains).await;
        let twice = deeper_than_alone::<{ 2 * HELD }>(&chains).await;
        // The stages' own frames are a fixed cost; the tail sits behind its box.
        assert!(
            held < HELD && held.abs_diff(twice) < DISPATCH_OVERHEAD,
            "a layered dispatch resumes a {HELD}-byte tail {held} bytes deeper than alone, \
             and one twice that size {twice} bytes deeper",
        );
    }

    #[test]
    fn a_dispatch_holds_the_tail_it_runs_once() {
        for chains in [bare(), gate_only(admitting, None)] {
            let mut view = TrailView::new();
            let mut cx = new_unit_context(&SEND, &mut view, None, None);
            let alone = std::mem::size_of_val(&Heavy::<HELD>::default().call(&mut cx));
            let dispatched =
                std::mem::size_of_val(&dispatch(&mut cx, &chains, &mut Heavy::<HELD>::default()));
            assert!(
                dispatched < alone + DISPATCH_OVERHEAD,
                "a {:?} dispatch weighs {dispatched} bytes around a {alone}-byte tail",
                chains.shape(),
            );
        }
    }

    #[test]
    fn a_dispatch_is_send_whatever_its_shape() {
        fn send<T: Send>(_: &T) {}
        let chains = bare();
        let mut view = TrailView::new();
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        send(&dispatch(&mut cx, &chains, &mut RecordingTail::ok()));
    }

    #[tokio::test]
    async fn a_global_filter_answers_after_the_methods_declined() {
        struct Answering;
        impl Layer for Answering {}

        #[async_trait]
        impl ExceptionFilter for Answering {
            type Exception = HandlerFailed;

            async fn catch(&self, _: &HandlerFailed, cx: &mut UnitContext<'_>) -> Caught {
                record(cx, "answered");
                Caught::Answered(settled())
            }
        }

        let container = ContainerBuilder::default()
            .provide(Declining)
            .provide(Answering)
            .provide(GlobalExceptionFilters(vec![
                exception_filter::<Answering>().0,
            ]))
            .build()
            .expect("the container builds");
        let chains = composed(
            &container,
            &SiteDeclaration {
                layers: || SiteLayers {
                    method_exception_filters: vec![exception_filter::<Declining>()],
                    ..SiteLayers::default()
                },
                global: GlobalLayers::ALL,
                guard_stage: None,
                class_gate: None,
            },
        );
        let (outcome, steps) = run(&chains, &mut RecordingTail::failing()).await;
        assert_eq!(steps, ["handler", "filter", "answered"]);
        assert!(outcome.is_err_and(|failure| failure.is_answered()));
    }
}
