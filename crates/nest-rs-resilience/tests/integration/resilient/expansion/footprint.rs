//! What a decorated method's future holds, and the stack it is polled on: its
//! body's future and each argument once, beside what its run holds whatever its
//! body, so a method holding a large buffer runs where its undecorated twin
//! runs.

use std::sync::Arc;

use nest_rs_core::{injectable, module};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};

use super::{Answer, CarrierDown, Courier, boot};
use crate::quiet;

/// What a body below holds across its `.await`, or takes by value: past
/// anything a run holds, so a second copy of it shows.
const HELD: usize = 16 * 1024;

/// What the rows under tokio's threshold hold or take: a quarter of it.
const UNDER: usize = 4 * 1024;

/// What a run holds whatever its body: its slot, its attempt's pass and
/// bounds, the timers it waits on.
const RUN: usize = 1024;

#[injectable]
struct BuffersService {
    #[inject]
    courier: Arc<ResiliencePolicy<Courier>>,
}

impl BuffersService {
    async fn holding<const N: usize>(&self) -> Answer<usize> {
        let buffer = [7_u8; N];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(courier)]
    async fn holding_once<const N: usize>(&self) -> Answer<usize> {
        let buffer = [7_u8; N];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(courier, idempotent)]
    async fn holding_again<const N: usize>(&self) -> Answer<usize> {
        let buffer = [7_u8; N];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    async fn taking<const N: usize>(&self, buffer: [u8; N]) -> Answer<usize> {
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(courier)]
    async fn taking_once<const N: usize>(&self, buffer: [u8; N]) -> Answer<usize> {
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(courier, idempotent)]
    async fn taking_again<const N: usize>(&self, buffer: [u8; N]) -> Answer<usize> {
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    /// The twin of the two methods below once their call failed: `sized`
    /// names `N`, which a fallback is handed no other way.
    async fn falling_back<const N: usize>(&self, sized: [(); N]) -> Answer<usize> {
        self.stale(sized, ResilienceError::Failed(CarrierDown))
            .await
    }

    #[resilient(courier, fallback = stale)]
    async fn falling_back_once<const N: usize>(&self, sized: [(); N]) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(courier, idempotent, fallback = stale)]
    async fn falling_back_again<const N: usize>(&self, sized: [(); N]) -> Answer<usize> {
        Err(CarrierDown)
    }

    async fn stale<const N: usize>(
        &self,
        _sized: [(); N],
        _error: ResilienceError<CarrierDown>,
    ) -> Answer<usize> {
        let buffer = [7_u8; N];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(courier)]
    async fn answering_once(&self) -> Answer<usize> {
        Ok(7)
    }

    #[resilient(courier, idempotent)]
    async fn answering_again(&self) -> Answer<usize> {
        Ok(7)
    }

    #[resilient(courier, cancel = token)]
    async fn naming_once(&self) -> Answer<usize> {
        drop(token);
        Ok(7)
    }

    #[resilient(courier, idempotent, cancel = token)]
    async fn naming_again(&self) -> Answer<usize> {
        drop(token);
        Ok(7)
    }
}

/// What each method below holds past the 16 KiB clippy's `large_futures`
/// allows a future a developer awaits.
const LADEN: usize = 32 * 1024;

/// The lint is denied on each shape whose large future the expansion
/// alone awaits — a body's, a fallback's, a run's whose predicates own
/// what they read — in a debug build, which boxes the first two, and in a
/// release one, which holds them in place.
#[deny(clippy::large_futures)]
impl BuffersService {
    #[resilient(courier)]
    async fn laden_once(&self) -> Answer<usize> {
        let buffer = [7_u8; LADEN];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(courier, idempotent)]
    async fn laden_again(&self) -> Answer<usize> {
        let buffer = [7_u8; LADEN];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(courier, fallback = unladen)]
    async fn unloaded_once(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    #[resilient(courier, idempotent, fallback = unladen)]
    async fn unloaded_again(&self) -> Answer<usize> {
        Err(CarrierDown)
    }

    async fn unladen(&self, _error: ResilienceError<CarrierDown>) -> Answer<usize> {
        let buffer = [7_u8; LADEN];
        tokio::task::yield_now().await;
        Ok(std::hint::black_box(&buffer).len())
    }

    #[resilient(
        courier,
        idempotent,
        retry_if = move |_error| ledger[0] == 0,
        record_if = move |_error| ledger[1] == 0,
        fallback = audited,
        fallback_if = move |_error| ledger[2] == 7,
    )]
    async fn audit(&self, ledger: [u8; LADEN]) -> Answer<usize> {
        Err(CarrierDown)
    }

    async fn audited(
        &self,
        ledger: [u8; LADEN],
        _error: ResilienceError<CarrierDown>,
    ) -> Answer<usize> {
        Ok(ledger.len())
    }
}

#[module(
    imports = [ResilienceModule::for_root((Courier, quiet()))],
    providers = [BuffersService],
)]
struct BuffersModule;

/// Each shape of a method holding or taking `N` bytes: the size of its
/// future decorated and undecorated, and what a debug build holds outside
/// its body's run — no buffer the body holds, the argument once that a
/// method run once moves into its body's closure, and the arguments as the
/// undecorated method holds them that a method run again lends each
/// attempt.
fn shapes<const N: usize>(
    buffers: &BuffersService,
) -> [(&'static str, usize, usize, usize, usize); 4] {
    let holding = size_of_val(&buffers.holding::<N>());
    let taking = size_of_val(&buffers.taking::<N>([7; N]));
    [
        (
            "holding a buffer across an `.await`, run once",
            N,
            size_of_val(&buffers.holding_once::<N>()),
            holding,
            0,
        ),
        (
            "holding a buffer across an `.await`, run again",
            N,
            size_of_val(&buffers.holding_again::<N>()),
            holding,
            0,
        ),
        (
            "taking a buffer by value, run once",
            N,
            size_of_val(&buffers.taking_once::<N>([7; N])),
            taking,
            N,
        ),
        (
            "taking a buffer by value, run again",
            N,
            size_of_val(&buffers.taking_again::<N>([7; N])),
            taking,
            taking,
        ),
    ]
}

/// A release build holds a decorated method's body and each argument once,
/// in place, beside what its run holds whatever its body; a debug build
/// boxes the body's closure and its future, so the method holds no more
/// than its arguments.
#[tokio::test]
async fn a_decorated_methods_future_holds_its_body_and_each_argument_once() {
    let app = boot::<BuffersModule>().await;
    let buffers = app.container().get::<BuffersService>().expect("provided");
    for (shape, bytes, decorated, twin, own) in shapes::<HELD>(&buffers)
        .into_iter()
        .chain(shapes::<UNDER>(&buffers))
    {
        if cfg!(debug_assertions) {
            assert!(
                decorated <= own + RUN,
                "in a debug build, a method {shape}, of {bytes} B, is a future of {decorated} \
                 B decorated, holding {own} B outside its run"
            );
        } else {
            assert!(
                twin <= decorated && decorated <= twin + RUN,
                "in a release build, a method {shape}, of {bytes} B, is a future of \
                 {decorated} B decorated, of {twin} B undecorated"
            );
        }
    }
}

/// A fallback's future is held as an attempt's is: in place in a release
/// build, once, as the method awaiting its fallback by hand holds it, and
/// boxed in a debug build, so the method's future holds none of it.
#[tokio::test]
async fn a_decorated_methods_future_holds_its_fallbacks_future_as_it_holds_its_bodys() {
    let app = boot::<BuffersModule>().await;
    let buffers = app.container().get::<BuffersService>().expect("provided");
    for (twin, once, again) in [
        (
            size_of_val(&buffers.falling_back([(); HELD])),
            size_of_val(&buffers.falling_back_once([(); HELD])),
            size_of_val(&buffers.falling_back_again([(); HELD])),
        ),
        (
            size_of_val(&buffers.falling_back([(); UNDER])),
            size_of_val(&buffers.falling_back_once([(); UNDER])),
            size_of_val(&buffers.falling_back_again([(); UNDER])),
        ),
    ] {
        for (shape, decorated) in [("run once", once), ("run again", again)] {
            if cfg!(debug_assertions) {
                assert!(
                    decorated <= RUN,
                    "in a debug build, a method {shape} whose fallback is a future of {twin} \
                     B is a future of {decorated} B"
                );
            } else {
                assert!(
                    twin <= decorated && decorated <= twin + RUN,
                    "in a release build, a method {shape} is a future of {decorated} B \
                     decorated, of {twin} B awaiting its fallback by hand"
                );
            }
        }
    }
}

/// Each shape `large_futures` is denied on answers: its body, its
/// fallback, and its fallback once its filter read what its predicates own.
#[tokio::test]
async fn a_method_whose_expansion_awaits_a_large_future_answers() {
    let app = boot::<BuffersModule>().await;
    let buffers = app.container().get::<BuffersService>().expect("provided");
    for (shape, answered) in [
        ("its body, run once", buffers.laden_once().await),
        ("its body, run again", buffers.laden_again().await),
        ("its fallback, run once", buffers.unloaded_once().await),
        ("its fallback, run again", buffers.unloaded_again().await),
        ("its predicates", buffers.audit([7; LADEN]).await),
    ] {
        assert!(matches!(answered, Ok(LADEN)), "{shape}: {answered:?}");
    }
}

/// tokio's default worker stack, which `#[nest_rs::main]`'s runtime keeps.
const WORKER_STACK: usize = 2 * 1024 * 1024;

/// What the methods below hold across their `.await` on that stack: a
/// third of it, which the undecorated method holds there too.
const HELD_ON_A_WORKER: usize = WORKER_STACK / 3;

/// A stack overflow aborts the process: nextest, which runs each test in a
/// process of its own, reports this one failed on `SIGABRT` and runs the
/// rest.
#[test]
fn a_decorated_method_holding_a_large_buffer_across_an_await_runs_on_a_worker_its_twin_runs_on() {
    // A runtime of the test's own, for its worker's stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .expect("the runtime builds");
    runtime.block_on(async {
        let app = boot::<BuffersModule>().await;
        let buffers = app.container().get::<BuffersService>().expect("provided");
        let twin = tokio::spawn({
            let buffers = Arc::clone(&buffers);
            async move { buffers.holding::<HELD_ON_A_WORKER>().await }
        });
        assert!(matches!(twin.await, Ok(Ok(HELD_ON_A_WORKER))));
        let once = tokio::spawn({
            let buffers = Arc::clone(&buffers);
            async move { buffers.holding_once::<HELD_ON_A_WORKER>().await }
        });
        assert!(matches!(once.await, Ok(Ok(HELD_ON_A_WORKER))));
        let again = tokio::spawn(async move { buffers.holding_again::<HELD_ON_A_WORKER>().await });
        assert!(matches!(again.await, Ok(Ok(HELD_ON_A_WORKER))));
    });
}

/// What the fallback below holds across its `.await` on that stack: a
/// fifth of it, which the method awaiting that fallback by hand holds
/// there too, under one `.await` more than the bodies above.
const HELD_BY_A_FALLBACK: usize = WORKER_STACK / 5;

/// A stack overflow aborts the process, as above.
#[test]
fn a_decorated_method_whose_fallback_holds_a_large_buffer_across_an_await_runs_on_a_worker_its_twin_runs_on()
 {
    // A runtime of the test's own, for its worker's stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .expect("the runtime builds");
    runtime.block_on(async {
        let app = boot::<BuffersModule>().await;
        let buffers = app.container().get::<BuffersService>().expect("provided");
        let twin = tokio::spawn({
            let buffers = Arc::clone(&buffers);
            async move { buffers.falling_back([(); HELD_BY_A_FALLBACK]).await }
        });
        assert!(matches!(twin.await, Ok(Ok(HELD_BY_A_FALLBACK))));
        let once = tokio::spawn({
            let buffers = Arc::clone(&buffers);
            async move { buffers.falling_back_once([(); HELD_BY_A_FALLBACK]).await }
        });
        assert!(matches!(once.await, Ok(Ok(HELD_BY_A_FALLBACK))));
        let again =
            tokio::spawn(async move { buffers.falling_back_again([(); HELD_BY_A_FALLBACK]).await });
        assert!(matches!(again.await, Ok(Ok(HELD_BY_A_FALLBACK))));
    });
}

/// What the methods below take by value on that stack: three thirty-seconds
/// of it. Run once, a decorated method moves what it takes through its
/// body's closure and into its future, each copy a debug build's stack holds
/// a moment, where its twin moves it once.
const TAKEN_ON_A_WORKER: usize = WORKER_STACK * 3 / 32;

/// A stack overflow aborts the process, as above.
#[test]
fn a_decorated_method_taking_a_large_buffer_by_value_runs_on_a_worker_of_tokios_default_stack() {
    // A runtime of the test's own, for its worker's stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .expect("the runtime builds");
    runtime.block_on(async {
        let app = boot::<BuffersModule>().await;
        let buffers = app.container().get::<BuffersService>().expect("provided");
        let once = tokio::spawn({
            let buffers = Arc::clone(&buffers);
            async move {
                buffers
                    .taking_once::<TAKEN_ON_A_WORKER>([7; TAKEN_ON_A_WORKER])
                    .await
            }
        });
        assert!(matches!(once.await, Ok(Ok(TAKEN_ON_A_WORKER))));
        let again = tokio::spawn(async move {
            buffers
                .taking_again::<TAKEN_ON_A_WORKER>([7; TAKEN_ON_A_WORKER])
                .await
        });
        assert!(matches!(again.await, Ok(Ok(TAKEN_ON_A_WORKER))));
    });
}

/// As a run's under tokio's threshold, a decorated method's call allocates
/// nothing of its own in a release build, and makes no cancellation token
/// for a body that names none: only one its `cancel` names is made, an
/// allocation. A debug build boxes its future, and its body's closure, which
/// allocates only when it holds something: run again it borrows the
/// method's own.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "pins the allocations a run makes on Linux, where the dev container and CI run it"
)]
async fn a_decorated_method_answering_at_once_allocates_nothing_but_the_token_it_names_in_a_release_build()
 {
    let app = boot::<BuffersModule>().await;
    let buffers = app.container().get::<BuffersService>().expect("provided");
    let token = crate::token_allocations();
    let debug = usize::from(cfg!(debug_assertions));
    for (shape, tokens, boxed, (answered, made)) in [
        (
            "run once",
            0,
            debug,
            crate::allocations(|| buffers.answering_once()),
        ),
        (
            "run again",
            0,
            2 * debug,
            crate::allocations(|| buffers.answering_again()),
        ),
        (
            "run once naming its token",
            1,
            debug,
            crate::allocations(|| buffers.naming_once()),
        ),
        (
            "run again naming its token",
            1,
            2 * debug,
            crate::allocations(|| buffers.naming_again()),
        ),
    ] {
        assert!(matches!(answered, Ok(7)), "{shape}: {answered:?}");
        assert_eq!(
            made,
            tokens * token + boxed,
            "a method {shape} allocates {tokens} token(s), each {token}, and in a debug build \
             {boxed} box(es)"
        );
    }
}
