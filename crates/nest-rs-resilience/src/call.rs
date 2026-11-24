//! What a run calls: its closure, then the future each attempt makes of it. A
//! release build holds each future where the run is, whatever its size, and
//! the closure there too, but for a call sent once whose closure is past
//! tokio's threshold, which it boxes with the future that closure makes. A
//! debug build boxes both. A run moves neither once it holds it, and drops the
//! closure as it answers.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use tokio_util::sync::CancellationToken;

use self::placed::{Placed, PlacedFuture, placed, placed_future, placed_future_mut, placed_mut};

/// Past this size a release build boxes the closure of a call sent once:
/// tokio's own threshold for a future it spawns in a release build, read off
/// tokio 1.54's `BOX_FUTURE_THRESHOLD` (`src/runtime/mod.rs`,
/// `.claude/upstream/tokio.md`).
const BOX_THRESHOLD: usize = 16384;

/// Whether a call sent once boxes its closure `T`, decided as tokio's own
/// `AutoBox` decides for a future it spawns: an associated constant, read once
/// per type, so only the branch taken is built.
struct AutoBox<T>(PhantomData<T>);

impl<T> AutoBox<T> {
    /// `true` if a value of type `T` is larger than [`BOX_THRESHOLD`].
    const SHOULD_BOX: bool = size_of::<T>() > BOX_THRESHOLD;
}

/// How a release build places a value in a run: in place.
#[cfg(not(debug_assertions))]
mod placed {
    use std::pin::Pin;

    pub(super) type Placed<T> = T;
    pub(super) type PlacedFuture<T> = T;

    pub(super) const fn placed<T>(value: T) -> Placed<T> {
        value
    }

    pub(super) const fn placed_mut<T>(placed: &mut Placed<T>) -> &mut T {
        placed
    }

    pub(super) const fn placed_future<T>(future: T) -> PlacedFuture<T> {
        future
    }

    pub(super) const fn placed_future_mut<T>(placed: Pin<&mut PlacedFuture<T>>) -> Pin<&mut T> {
        placed
    }
}

/// How a debug build places a value in a run: boxed.
// A debug build copies a value's whole type at each move, and a place that may hold
// a large future has its size (`.claude/upstream/rustc.md`).
#[cfg(debug_assertions)]
mod placed {
    use std::pin::Pin;

    pub(super) type Placed<T> = Box<T>;
    pub(super) type PlacedFuture<T> = Pin<Box<T>>;

    pub(super) fn placed<T>(value: T) -> Placed<T> {
        Box::new(value)
    }

    pub(super) fn placed_mut<T>(placed: &mut Placed<T>) -> &mut T {
        placed
    }

    pub(super) fn placed_future<T>(future: T) -> PlacedFuture<T> {
        Box::pin(future)
    }

    pub(super) fn placed_future_mut<T>(placed: Pin<&mut PlacedFuture<T>>) -> Pin<&mut T> {
        placed.get_mut().as_mut()
    }
}

/// A run's call, held while each of its attempts makes its future and polls
/// it: once by the run's one attempt, or again by each.
pub(crate) trait Call {
    /// The future an attempt sends.
    type Made: Future;

    /// The future of the next attempt, handed its token, held by the call.
    fn make(self: Pin<&mut Self>, cancel: CancellationToken);

    /// The future the attempt made.
    fn made(self: Pin<&mut Self>) -> Pin<&mut Self::Made>;

    /// The attempt's future dropped once it ends.
    fn spend(self: Pin<&mut Self>);

    /// The closure dropped once the run answers, with what it captured.
    fn release(self: Pin<&mut Self>);
}

pin_project_lite::pin_project! {
    /// A call its run's one attempt makes, consuming the closure: the closure
    /// until then and the future it made after, sharing one place. Past the
    /// threshold, a release build boxes both, since the closure leaves its box
    /// to be called and a future built beside it to be moved into place would
    /// take its size again; the place they share still reserves the larger of
    /// their two sizes beside the boxes.
    pub(crate) struct Once<F, Fut> {
        #[pin]
        placed: OncePlaced<F, Fut>,
        boxed_call: Option<Box<F>>,
        boxed_made: Option<Pin<Box<Fut>>>,
    }
}

pin_project_lite::pin_project! {
    #[project = OncePlacedProj]
    enum OncePlaced<F, Fut> {
        Call {
            call: Option<Placed<F>>,
        },
        Made {
            #[pin]
            made: PlacedFuture<Fut>,
        },
        Spent,
    }
}

impl<F, Fut> Once<F, Fut>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future,
{
    pub(crate) fn new(call: F) -> Self {
        if AutoBox::<F>::SHOULD_BOX {
            Self {
                placed: OncePlaced::Call { call: None },
                boxed_call: Some(Box::new(call)),
                boxed_made: None,
            }
        } else {
            Self {
                placed: OncePlaced::Call {
                    call: Some(placed(call)),
                },
                boxed_call: None,
                boxed_made: None,
            }
        }
    }
}

impl<F, Fut> Call for Once<F, Fut>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future,
{
    type Made = Fut;

    fn make(self: Pin<&mut Self>, cancel: CancellationToken) {
        let mut this = self.project();
        if AutoBox::<F>::SHOULD_BOX {
            let Some(call) = this.boxed_call.take() else {
                unreachable!("a call made once is made by its run's one attempt");
            };
            *this.boxed_made = Some(Box::pin(call(cancel)));
        } else {
            let OncePlacedProj::Call { call } = this.placed.as_mut().project() else {
                unreachable!("a call made once is made by its run's one attempt");
            };
            let Some(call) = call.take() else {
                unreachable!("a call made once is made by its run's one attempt");
            };
            this.placed.set(OncePlaced::Made {
                made: placed_future(call(cancel)),
            });
        }
    }

    fn made(self: Pin<&mut Self>) -> Pin<&mut Fut> {
        let this = self.project();
        if AutoBox::<F>::SHOULD_BOX {
            let Some(made) = this.boxed_made else {
                unreachable!("an attempt polls the future its call made");
            };
            made.as_mut()
        } else {
            let OncePlacedProj::Made { made } = this.placed.project() else {
                unreachable!("an attempt polls the future its call made");
            };
            placed_future_mut(made)
        }
    }

    fn spend(self: Pin<&mut Self>) {
        let mut this = self.project();
        if AutoBox::<F>::SHOULD_BOX {
            *this.boxed_made = None;
        } else {
            this.placed.set(OncePlaced::Spent);
        }
    }

    fn release(self: Pin<&mut Self>) {
        let this = self.project();
        if AutoBox::<F>::SHOULD_BOX {
            *this.boxed_call = None;
        } else if let OncePlacedProj::Call { call } = this.placed.project() {
            *call = None;
        }
    }
}

pin_project_lite::pin_project! {
    /// A call each attempt of its run makes, the closure held throughout and
    /// each attempt's future after it. A release build holds both where the
    /// run is, whatever their size: the closure is called where it is held,
    /// and the run's type reserves its size either way, so a box would buy no
    /// stack.
    pub(crate) struct Again<F, Fut> {
        call: Option<Placed<F>>,
        #[pin]
        made: Option<PlacedFuture<Fut>>,
    }
}

impl<F, Fut> Again<F, Fut>
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: Future,
{
    pub(crate) fn new(call: F) -> Self {
        Self {
            call: Some(placed(call)),
            made: None,
        }
    }
}

impl<F, Fut> Call for Again<F, Fut>
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: Future,
{
    type Made = Fut;

    fn make(self: Pin<&mut Self>, cancel: CancellationToken) {
        let mut this = self.project();
        let Some(call) = this.call.as_mut().map(placed_mut) else {
            unreachable!("a call made again holds its closure until its run answers");
        };
        this.made.set(Some(placed_future(call(cancel))));
    }

    fn made(self: Pin<&mut Self>) -> Pin<&mut Fut> {
        let Some(made) = self.project().made.as_pin_mut() else {
            unreachable!("an attempt polls the future its call made");
        };
        placed_future_mut(made)
    }

    fn spend(self: Pin<&mut Self>) {
        self.project().made.set(None);
    }

    fn release(self: Pin<&mut Self>) {
        *self.project().call = None;
    }
}
