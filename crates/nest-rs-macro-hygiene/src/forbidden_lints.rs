//! A crate forbidding a lint refuses every `#[allow]` of it written inside, an
//! expansion's included (`E0453`), and fails on every warning it raises. One
//! impl half compiles here, `#[scheduled]`: its methods, in every answer its
//! probe reads, under the lints an answer of `!` trips where it is bound or
//! read, and the one an `.await` of a large future trips outside a macro.

#![forbid(
    unreachable_code,
    clippy::diverging_sub_expression,
    clippy::large_futures
)]

/// A tick of each answer `#[scheduled]` reads — `anyhow::Result<()>`, an error
/// of its own, and `!` written out, `async` or not, through the projection's
/// `Never` and through a `ty` fragment — and one whose future is past
/// `large_futures`' threshold, which only the expansion awaits.
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
