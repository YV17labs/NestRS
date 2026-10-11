//! Doubles for this crate's unit tests: a fake edge whose view keeps a trail
//! of what ran, the layers that write to it, and an allocator counting what a
//! test's code allocates on its thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::any::TypeId;
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;

use crate::dispatch::{GuardStage, Tail};
use crate::handler::{Handler, HandlerDeclaration, MetaLevels, Posture, declare_handler};
use crate::layer::Layer;
use crate::operation_log::{__private::declare_unit, Kind, Unit};
use crate::outcome::{__private::settled, Failure, Outcome};
use crate::unit_context::{UnitContext, UnitView};
use crate::{Interceptor, Next};

/// The unit the fake edge dispatches.
pub(crate) const MESSAGE: Unit =
    declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message");

/// A unit of another edge, which a layer may decline to serve.
pub(crate) const JOB: Unit = declare_unit(
    "nest-rs-queue",
    "nest_rs::queue",
    Kind::Consumer,
    "queue.job",
);

struct ChatGateway;

/// The site the fake edge dispatches to.
pub(crate) static SEND: Handler = declare_handler(HandlerDeclaration {
    unit: MESSAGE,
    host: TypeId::of::<ChatGateway>(),
    host_name: "ChatGateway",
    method: "send",
    address: "send",
    posture: Posture::Gated,
    meta: MetaLevels::default,
});

struct Jobs;

/// A site of another edge.
pub(crate) static RUN: Handler = declare_handler(HandlerDeclaration {
    unit: JOB,
    host: TypeId::of::<Jobs>(),
    host_name: "Jobs",
    method: "run",
    address: "jobs",
    posture: Posture::System,
    meta: MetaLevels::default,
});

/// What ran, in order, as each layer wrote it.
#[derive(Default)]
pub(crate) struct Trail(Mutex<Vec<String>>);

impl Trail {
    pub(crate) fn push(&self, step: impl Into<String>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(step.into());
    }

    pub(crate) fn steps(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The fake edge's view: the trail every layer of the unit writes to.
pub(crate) struct TrailView(pub(crate) Arc<Trail>);

impl UnitView for TrailView {}

impl TrailView {
    pub(crate) fn new() -> Self {
        Self(Arc::default())
    }

    pub(crate) fn steps(&self) -> Vec<String> {
        self.0.steps()
    }
}

/// Write `step` to the trail of the unit `cx` describes.
pub(crate) fn record(cx: &UnitContext<'_>, step: impl Into<String>) {
    if let Some(view) = cx.switch_to::<TrailView>() {
        view.0.push(step);
    }
}

/// An interceptor writing its name around the rest of the unit; `M` gives
/// each one a type of its own.
pub(crate) struct Recording<M> {
    name: &'static str,
    priority: i8,
    serves: Option<Unit>,
    marker: PhantomData<fn() -> M>,
}

impl<M> Recording<M> {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            priority: 0,
            serves: None,
            marker: PhantomData,
        }
    }

    pub(crate) const fn with_priority(self, priority: i8) -> Self {
        Self { priority, ..self }
    }

    /// Serving `unit` alone.
    pub(crate) const fn serving(self, unit: Unit) -> Self {
        Self {
            serves: Some(unit),
            ..self
        }
    }
}

impl<M: 'static> Layer for Recording<M> {
    fn priority(&self) -> i8 {
        self.priority
    }

    fn serves(&self, unit: Unit) -> bool {
        self.serves.is_none_or(|served| served == unit)
    }
}

#[async_trait]
impl<M: 'static> Interceptor for Recording<M> {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        record(cx, format!("{} in", self.name));
        let outcome = next.run(cx).await;
        record(cx, format!("{} out", self.name));
        outcome
    }
}

/// A guard stage writing `guards` and admitting, or denying with `Refused`.
pub(crate) struct RecordingGuards {
    pub(crate) admit: bool,
}

/// What a denying guard stage fails with.
#[derive(Debug, thiserror::Error)]
#[error("the guards refused the unit")]
pub(crate) struct Refused;

impl GuardStage for RecordingGuards {
    fn check<'a>(
        &'a self,
        cx: &'a mut UnitContext<'_>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), Failure>> + Send + 'a>> {
        Box::pin(async move {
            record(cx, "guards");
            if self.admit {
                Ok(())
            } else {
                Err(Failure::from(Refused).answered(settled()))
            }
        })
    }
}

/// The class gate, writing `gate` and admitting.
pub(crate) fn recording_gate(cx: &mut UnitContext<'_>) -> Result<(), Failure> {
    record(cx, "gate");
    Ok(())
}

/// The handler, writing `handler` and coming to `outcome`.
pub(crate) struct RecordingTail(pub(crate) fn() -> Outcome);

impl RecordingTail {
    pub(crate) fn ok() -> Self {
        Self(|| Ok(settled()))
    }

    pub(crate) fn failing() -> Self {
        Self(|| Err(Failure::from(HandlerFailed)))
    }
}

/// What a failing handler fails with.
#[derive(Debug, thiserror::Error)]
#[error("the handler failed")]
pub(crate) struct HandlerFailed;

impl Tail for RecordingTail {
    async fn call(&mut self, cx: &mut UnitContext<'_>) -> Outcome {
        record(cx, "handler");
        (self.0)()
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The system allocator, counting the allocations made on a thread while it
/// is asked to.
struct Counting;

thread_local! {
    /// The allocations made on this thread since counting began, `None` while
    /// nothing counts.
    static COUNTED: Cell<Option<usize>> = const { Cell::new(None) };
}

fn count() {
    // Off a thread's storage, as it is torn down, nothing is being counted.
    if let Ok(Some(made)) = COUNTED.try_with(Cell::get) {
        COUNTED.set(Some(made + 1));
    }
}

#[expect(
    unsafe_code,
    reason = "an allocator is an unsafe trait; each call goes to the system allocator unchanged"
)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Poll `fut` until it is ready, on this thread, counting what each poll
/// allocates: what the future returns, and how many allocations it made.
pub(crate) fn run_counting<F: Future>(fut: F) -> (F::Output, usize) {
    let mut fut = std::pin::pin!(fut);
    let mut poll = Context::from_waker(Waker::noop());
    let mut made = 0;
    loop {
        COUNTED.set(Some(made));
        let polled = fut.as_mut().poll(&mut poll);
        made = COUNTED.replace(None).unwrap_or(made);
        if let Poll::Ready(output) = polled {
            return (output, made);
        }
    }
}
