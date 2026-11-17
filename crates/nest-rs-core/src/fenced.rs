//! [`Fenced`], the subscriber every installer of the framework's sets as the
//! process's global one: it counts how deep each thread is inside it, so the
//! panic hook can tell a panic raised inside a subscriber's own dispatch
//! without tracing-core discarding every line filed there.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    /// How many of [`Fenced`]'s methods this thread is inside.
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Whether this thread is inside a fenced subscriber's own dispatch.
pub(crate) fn inside() -> bool {
    DEPTH.with(|depth| depth.get() > 0)
}

/// Set once an installer of the framework's made a fenced subscriber the
/// process's global one.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Whether the process's global subscriber is fenced.
pub(crate) fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// Record that a fenced subscriber is the process's global one.
#[cfg(feature = "logging")]
pub(crate) fn mark_installed() {
    INSTALLED.store(true, Ordering::Relaxed);
}

#[cfg(feature = "logging")]
pub(crate) use subscriber::Fenced;

#[cfg(feature = "logging")]
mod subscriber {
    use std::any::TypeId;

    use tracing::level_filters::LevelFilter;
    use tracing::subscriber::Interest;
    use tracing::{Dispatch, Event, Metadata, Subscriber, span};

    use super::DEPTH;

    /// `S`, every method of [`Subscriber`] handed to it while this thread's
    /// depth counts the call.
    pub(crate) struct Fenced<S>(S);

    impl<S> Fenced<S> {
        pub(crate) fn new(subscriber: S) -> Self {
            Self(subscriber)
        }
    }

    /// One call inside the fence, counted until it returns or unwinds.
    struct Inside;

    impl Inside {
        fn enter() -> Self {
            DEPTH.with(|depth| depth.set(depth.get() + 1));
            Self
        }
    }

    impl Drop for Inside {
        fn drop(&mut self) {
            DEPTH.with(|depth| depth.set(depth.get() - 1));
        }
    }

    impl<S: Subscriber> Subscriber for Fenced<S> {
        fn on_register_dispatch(&self, subscriber: &Dispatch) {
            let _inside = Inside::enter();
            self.0.on_register_dispatch(subscriber);
        }

        fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
            let _inside = Inside::enter();
            self.0.register_callsite(metadata)
        }

        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            let _inside = Inside::enter();
            self.0.enabled(metadata)
        }

        fn max_level_hint(&self) -> Option<LevelFilter> {
            let _inside = Inside::enter();
            self.0.max_level_hint()
        }

        fn new_span(&self, span: &span::Attributes<'_>) -> span::Id {
            let _inside = Inside::enter();
            self.0.new_span(span)
        }

        fn record(&self, span: &span::Id, values: &span::Record<'_>) {
            let _inside = Inside::enter();
            self.0.record(span, values);
        }

        fn record_follows_from(&self, span: &span::Id, follows: &span::Id) {
            let _inside = Inside::enter();
            self.0.record_follows_from(span, follows);
        }

        fn event_enabled(&self, event: &Event<'_>) -> bool {
            let _inside = Inside::enter();
            self.0.event_enabled(event)
        }

        fn event(&self, event: &Event<'_>) {
            let _inside = Inside::enter();
            self.0.event(event);
        }

        fn enter(&self, span: &span::Id) {
            let _inside = Inside::enter();
            self.0.enter(span);
        }

        fn exit(&self, span: &span::Id) {
            let _inside = Inside::enter();
            self.0.exit(span);
        }

        fn clone_span(&self, id: &span::Id) -> span::Id {
            let _inside = Inside::enter();
            self.0.clone_span(id)
        }

        fn drop_span(&self, id: span::Id) {
            let _inside = Inside::enter();
            #[expect(
                deprecated,
                reason = "every method is handed on, the one tracing-core retired included"
            )]
            self.0.drop_span(id);
        }

        fn try_close(&self, id: span::Id) -> bool {
            let _inside = Inside::enter();
            self.0.try_close(id)
        }

        fn current_span(&self) -> tracing_core::span::Current {
            let _inside = Inside::enter();
            self.0.current_span()
        }

        // The trace layer and the linker downcast the global dispatch to the
        // layers `S` holds, so every type but the fence's own is `S`'s answer.
        #[expect(
            unsafe_code,
            reason = "the trait's one downcast is an unsafe fn, handed on unchanged"
        )]
        unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
            if id == TypeId::of::<Self>() {
                return Some(std::ptr::from_ref(self).cast());
            }
            let _inside = Inside::enter();
            // SAFETY: `S` answers for its own types, under the contract this
            // method was called with.
            unsafe { self.0.downcast_raw(id) }
        }
    }
}
