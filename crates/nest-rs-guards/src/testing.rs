//! Doubles for this crate's unit tests: an allocator counting what a test's
//! code allocates on its thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;
use std::task::{Context, Poll, Waker};

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

/// Build `fut` and poll it until it is ready, on this thread, counting what the
/// building and each poll allocate: what the future returns, and how many
/// allocations it made.
pub(crate) fn run_counting<F: Future>(fut: impl FnOnce() -> F) -> (F::Output, usize) {
    COUNTED.set(Some(0));
    let fut = fut();
    let mut made = COUNTED.replace(None).unwrap_or(0);
    let mut fut = std::pin::pin!(fut);
    let mut poll = Context::from_waker(Waker::noop());
    loop {
        COUNTED.set(Some(made));
        let polled = fut.as_mut().poll(&mut poll);
        made = COUNTED.replace(None).unwrap_or(made);
        if let Poll::Ready(output) = polled {
            return (output, made);
        }
    }
}
