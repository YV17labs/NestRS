//! What a run calls: its closure, then the future each attempt makes of it. A
//! release build holds each future where the run is, whatever its size, and the
//! closure there up to tokio's threshold; past it, it boxes the closure, and in
//! a call sent once the future that closure makes. A debug build boxes both. A
//! run moves neither once it holds it.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use tokio_util::sync::CancellationToken;

use self::placed::{Placed, PlacedFuture, borrowed, pinned, placed, placed_future};

/// Past this size a release build boxes a run's closure: tokio's own threshold
/// for a future it spawns in a release build, read off tokio 1.54's
/// `BOX_FUTURE_THRESHOLD` (`src/runtime/mod.rs`, `.claude/upstream/tokio.md`).
const BOX_THRESHOLD: usize = 16384;

/// Whether a run places a closure `T` with the rest of what it holds, an
/// associated constant as tokio's `AutoBox` is: read once per type, so only the
/// branch taken is built.
struct Placement<T>(PhantomData<T>);

impl<T> Placement<T> {
    const FITS: bool = size_of::<T>() <= BOX_THRESHOLD;
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

    pub(super) const fn borrowed<T>(placed: &mut Placed<T>) -> &mut T {
        placed
    }

    pub(super) const fn placed_future<T>(future: T) -> PlacedFuture<T> {
        future
    }

    pub(super) const fn pinned<T>(placed: Pin<&mut PlacedFuture<T>>) -> Pin<&mut T> {
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

    pub(super) fn borrowed<T>(placed: &mut Placed<T>) -> &mut T {
        placed
    }

    pub(super) fn placed_future<T>(future: T) -> PlacedFuture<T> {
        Box::pin(future)
    }

    pub(super) fn pinned<T>(placed: Pin<&mut PlacedFuture<T>>) -> Pin<&mut T> {
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
}

pin_project_lite::pin_project! {
    /// A call its run's one attempt makes, consuming the closure: the closure
    /// until then and the future it made after. Up to the threshold, a release
    /// build holds both in one place, the future whatever its size; past it, it
    /// boxes both, since the closure leaves its box to be called and a future
    /// built beside it to be moved into place would take its size again.
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
        if Placement::<F>::FITS {
            Self {
                placed: OncePlaced::Call {
                    call: Some(placed(call)),
                },
                boxed_call: None,
                boxed_made: None,
            }
        } else {
            Self {
                placed: OncePlaced::Call { call: None },
                boxed_call: Some(Box::new(call)),
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
        if Placement::<F>::FITS {
            let OncePlacedProj::Call { call } = this.placed.as_mut().project() else {
                unreachable!("a call made once is made by its run's one attempt");
            };
            let Some(call) = call.take() else {
                unreachable!("a call made once is made by its run's one attempt");
            };
            this.placed.set(OncePlaced::Made {
                made: placed_future(call(cancel)),
            });
        } else {
            let Some(call) = this.boxed_call.take() else {
                unreachable!("a call made once is made by its run's one attempt");
            };
            *this.boxed_made = Some(Box::pin(call(cancel)));
        }
    }

    fn made(self: Pin<&mut Self>) -> Pin<&mut Fut> {
        let this = self.project();
        if Placement::<F>::FITS {
            let OncePlacedProj::Made { made } = this.placed.project() else {
                unreachable!("an attempt polls the future its call made");
            };
            pinned(made)
        } else {
            let Some(made) = this.boxed_made else {
                unreachable!("an attempt polls the future its call made");
            };
            made.as_mut()
        }
    }

    fn spend(self: Pin<&mut Self>) {
        let mut this = self.project();
        if Placement::<F>::FITS {
            this.placed.set(OncePlaced::Spent);
        } else {
            *this.boxed_made = None;
        }
    }
}

pin_project_lite::pin_project! {
    /// A call each attempt of its run makes, the closure held throughout and
    /// each attempt's future after it. A release build holds each future where
    /// the run is whatever its size, and the closure up to the threshold, boxed
    /// past it.
    pub(crate) struct Again<F, Fut> {
        call: Option<Placed<F>>,
        boxed_call: Option<Box<F>>,
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
        if Placement::<F>::FITS {
            Self {
                call: Some(placed(call)),
                boxed_call: None,
                made: None,
            }
        } else {
            Self {
                call: None,
                boxed_call: Some(Box::new(call)),
                made: None,
            }
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
        let call = if Placement::<F>::FITS {
            this.call.as_mut().map(borrowed)
        } else {
            this.boxed_call.as_deref_mut()
        };
        let Some(call) = call else {
            unreachable!("a call made again holds its closure throughout its run");
        };
        this.made.set(Some(placed_future(call(cancel))));
    }

    fn made(self: Pin<&mut Self>) -> Pin<&mut Fut> {
        let Some(made) = self.project().made.as_pin_mut() else {
            unreachable!("an attempt polls the future its call made");
        };
        pinned(made)
    }

    fn spend(self: Pin<&mut Self>) {
        self.project().made.set(None);
    }
}
