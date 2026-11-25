//! A crate forbidding a lint refuses every `#[allow]` of it written inside, an
//! expansion's included (`E0453`), and fails on every warning it raises. One
//! impl half compiles here, `#[scheduled]`: its methods, in every answer its
//! probe reads, under the lints an answer of `!` trips where it is bound or
//! read, and the one an `.await` of a large future trips outside a macro. One
//! method decorator does too, `#[resilient]`, in each form its keys expand to,
//! whichever of its futures is the large one.

#![forbid(
    unreachable_code,
    clippy::diverging_sub_expression,
    clippy::large_futures
)]

/// A tick of each answer `#[scheduled]` reads — `anyhow::Result<()>`, an error
/// of its own, and `!` written out, `async` or not, through the projection's
/// `Never` and through a `ty` fragment — and one whose future is past
/// `large_futures`' threshold, which only the expansion awaits.
///
/// Each tick answering `!` compiles in a crate of edition 2021 too, under the
/// same lints, where a diverging expression's type falls back to `()` rather
/// than `!`; the example is that crate:
///
/// ```edition2021
/// #![forbid(unreachable_code, clippy::diverging_sub_expression)]
///
/// use nest_rs::core::injectable;
/// use nest_rs::schedule::scheduled;
/// use nest_rs_macro_hygiene::never::Never;
///
/// #[injectable]
/// pub struct HygieneEdition2021Tasks;
///
/// #[scheduled]
/// impl HygieneEdition2021Tasks {
///     #[after("1s")]
///     fn halt(&self) -> ! {
///         loop {
///             std::thread::park();
///         }
///     }
///
///     #[after("1s")]
///     async fn pump(&self) -> ! {
///         std::future::pending().await
///     }
///
///     #[after("1s")]
///     fn drain(&self) -> Never {
///         loop {
///             std::thread::park();
///         }
///     }
/// }
///
/// macro_rules! declare_fragment_tasks {
///     ($answer:ty) => {
///         #[injectable]
///         pub struct HygieneEdition2021FragmentTasks;
///
///         #[scheduled]
///         impl HygieneEdition2021FragmentTasks {
///             #[after("1s")]
///             fn halt(&self) -> $answer {
///                 loop {
///                     std::thread::park();
///                 }
///             }
///         }
///     };
/// }
///
/// declare_fragment_tasks!(!);
///
/// fn main() {}
/// ```
#[cfg(feature = "schedule")]
pub mod scheduled {
    use nest_rs::core::injectable;
    use nest_rs::schedule::scheduled;

    use crate::never::Never;

    /// What the sweep answers when the store refuses it: an error of its own.
    #[derive(Debug)]
    pub struct HygieneSweepRefused;

    impl std::fmt::Display for HygieneSweepRefused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the store refused the sweep")
        }
    }

    impl std::error::Error for HygieneSweepRefused {}

    #[injectable]
    pub struct HygieneAnswerTasks;

    #[scheduled]
    impl HygieneAnswerTasks {
        #[every("1m")]
        async fn report(&self) -> nest_rs::core::anyhow::Result<()> {
            Ok(())
        }

        #[every("1m")]
        fn sweep(&self) -> Result<(), HygieneSweepRefused> {
            Err(HygieneSweepRefused)
        }

        #[after("1s")]
        fn halt(&self) -> ! {
            loop {
                std::thread::park();
            }
        }

        #[after("1s")]
        async fn pump(&self) -> ! {
            std::future::pending().await
        }

        #[after("1s")]
        fn drain(&self) -> Never {
            loop {
                std::thread::park();
            }
        }

        /// Holds a buffer past clippy's 16 KiB across an `.await`.
        #[every("1h")]
        async fn archive(&self) -> nest_rs::core::anyhow::Result<()> {
            let buffer = [0_u8; 32 * 1024];
            std::future::ready(()).await;
            std::hint::black_box(&buffer);
            Ok(())
        }
    }

    // A `ty` fragment wraps the `!` in an invisible group.
    macro_rules! declare_fragment_tasks {
        ($answer:ty) => {
            #[injectable]
            pub struct HygieneFragmentTasks;

            #[scheduled]
            impl HygieneFragmentTasks {
                #[after("1s")]
                fn halt(&self) -> $answer {
                    loop {
                        std::thread::park();
                    }
                }
            }
        };
    }

    declare_fragment_tasks!(!);
}

/// A method of each form `#[resilient]` expands to — run once, its one attempt
/// owning the arguments or lending them to a predicate, sent again, answered
/// by a fallback its filter chooses, its token read — each opening on a
/// `return` never taken, and each future only the expansion awaits past
/// `large_futures`' threshold: a body's, a fallback's under a method run once
/// and again, and a run's whose predicates own what they read. A release
/// build holds a body's and a fallback's in place, where `nest-rs-resilience`'s
/// own suite denies the lint on the same shapes, its clippy run in both
/// profiles. The lints a binding nothing reads trips are forbidden on the
/// methods beside the file's, and so is the one braces around one expression
/// trip: a body written on one line is kept in braces of the expansion's.
#[cfg(feature = "resilience")]
pub mod resilient {
    use std::sync::Arc;

    use nest_rs::core::injectable;
    use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

    /// What the depot answers when it fails.
    #[derive(Debug)]
    pub struct HygieneDepotDown;

    impl std::fmt::Display for HygieneDepotDown {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the depot did not answer")
        }
    }

    impl std::error::Error for HygieneDepotDown {}

    type Answer<T> = Result<T, ResilienceError<HygieneDepotDown>>;

    #[injectable]
    pub struct HygieneForbiddingDepot {
        #[inject]
        depot: Arc<ResiliencePolicy>,
    }

    #[forbid(unused_variables, unused_mut, unused_braces)]
    impl HygieneForbiddingDepot {
        #[rustfmt::skip]
        #[resilient(depot)]
        pub async fn count(&self) -> Answer<usize> { Ok(0) }

        #[rustfmt::skip]
        #[resilient(depot, idempotent, fallback = recalled, cancel = token)]
        pub async fn recount(&self) -> Answer<usize> { Ok(usize::from(token.is_cancelled())) }

        #[resilient(depot)]
        pub async fn store(&self, mut parcel: String) -> Answer<usize> {
            parcel.push('!');
            Ok(parcel.len())
        }

        #[resilient(depot, record_if = |_error| !parcel.is_empty(), cancel = token)]
        pub async fn weigh(&self, parcel: String) -> Answer<usize> {
            if token.is_cancelled() {
                return Err(HygieneDepotDown);
            }
            Ok(parcel.len())
        }

        #[resilient(
            depot,
            idempotent,
            timeout = "2s",
            retries = 2,
            retry_if = |_error| !parcel.is_empty(),
            retry_ok_if = |weight| *weight == 0,
            cancel = token
        )]
        pub async fn locate(&self, parcel: String) -> Answer<usize> {
            if token.is_cancelled() {
                return Err(HygieneDepotDown);
            }
            Ok(parcel.len())
        }

        #[resilient(
            depot,
            fallback = last_known,
            fallback_if = |_error| !parcel.is_empty(),
            cancel = token
        )]
        pub async fn track(&self, parcel: String) -> Answer<usize> {
            drop(token);
            Ok(parcel.len())
        }

        async fn last_known(
            &self,
            parcel: String,
            _error: ResilienceError<HygieneDepotDown>,
        ) -> Answer<usize> {
            Ok(parcel.len())
        }

        /// Holds a buffer past clippy's 16 KiB across an `.await`.
        #[resilient(depot, idempotent, fallback = unarchived)]
        pub async fn archive(&self) -> Answer<usize> {
            let buffer = [0_u8; 32 * 1024];
            std::future::ready(()).await;
            std::hint::black_box(&buffer);
            Ok(buffer.len())
        }

        async fn unarchived(&self, _error: ResilienceError<HygieneDepotDown>) -> Answer<usize> {
            Ok(0)
        }

        #[resilient(depot, fallback = recalled)]
        pub async fn recall(&self) -> Answer<usize> {
            Ok(0)
        }

        #[resilient(depot, idempotent, fallback = recalled)]
        pub async fn recall_again(&self) -> Answer<usize> {
            Ok(0)
        }

        /// Holds a buffer past clippy's 16 KiB across an `.await`.
        async fn recalled(&self, _error: ResilienceError<HygieneDepotDown>) -> Answer<usize> {
            let buffer = [0_u8; 32 * 1024];
            std::future::ready(()).await;
            std::hint::black_box(&buffer);
            Ok(buffer.len())
        }

        /// Each predicate owns a copy of a ledger past clippy's 16 KiB.
        #[resilient(
            depot,
            idempotent,
            retry_if = move |_error| ledger[0] == 0,
            record_if = move |_error| ledger[1] == 0,
            fallback = audited,
            fallback_if = move |_error| ledger[2] == 0,
        )]
        pub async fn audit(&self, ledger: [u8; 32 * 1024]) -> Answer<usize> {
            Ok(ledger.len())
        }

        async fn audited(
            &self,
            ledger: [u8; 32 * 1024],
            _error: ResilienceError<HygieneDepotDown>,
        ) -> Answer<usize> {
            Ok(ledger.len())
        }
    }
}
