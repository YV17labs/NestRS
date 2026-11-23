# rustc

## An async closure's body is not coerced to its return type

- **Seen** — 1.100.0 (`0ef1bc386 2026-11-09`, the toolchain
  `rust-toolchain.toml` pins), editions 2021 and 2024: the Ticket's program,
  saved as `ticket.rs` and built alone outside the workspace with
  `rustc +1.100 --edition 2021 ticket.rs`, then `--edition 2024`. Its `async fn`
  and its closure that is not async, each declaring `Result<&str, ()>`, coerce a
  `&String` into the `&str` at their tail and at an early `return`. The same
  body in an async closure is coerced at neither, whether the closure declares
  its return type or rustc deduces it from the bound the closure is handed to,
  `F: FnOnce() -> Fut, Fut: Future<Output = Result<&'a str, ()>>`, the shape
  `ResiliencePolicy::run` takes. rustc refuses the program with six `E0308`, one
  at each line its comments mark, in both editions alike; the first:

  ```text
  error[E0308]: mismatched types
    --> ticket.rs:30:45
     |
  30 |     let tail = async || -> Result<&str, ()> { Ok(&hub.0) }; // E0308
     |                                             ^^^^^^^^^^^^^^ expected `str`, found `String`
     |
     = note: expected `async` closure body `{async closure body@ticket.rs:30:45: 30:59}` (`str`)
                found `async` closure body `{async closure body@ticket.rs:30:45: 30:59}` (`String`)
  ```

  At an early `return`, that `return` types the body as `Result<&String, _>`,
  the tail's `Ok("elsewhere")` is refused against it
  (`expected &String, found &str`), and the body as above: rustc checks it as an
  async block whose output it infers, then equates that output with the type
  declared or deduced, making no coercion. With each `Ok(&hub.0)` written
  `Ok(hub.0.as_str())`, the program builds in both editions without a warning:
  the coercion is all rustc refuses.
- **Effect** — `#[resilient]` cannot hand a method's body to its run as an async
  closure declaring the attempt's type: an answer the undecorated method coerces
  — `Ok(&self.hub)` for a `&str`, `Ok(Box::new(…))` for a `Box<dyn …>` — would
  not compile under the decorator.
- **Our side** — the body stays an async block, opening after its leading items
  on a `return` of the attempt's type that is never taken,
  `if false { return ::nest_rs_resilience::__private::attempt::<Answer>(); }`
  (`body` in `crates/nest-rs-resilience-macros/src/resilient.rs`): an async
  block's first `return` sets its output type, so its tail and every later
  `return` are checked, and coerced, against it. The async closure was not taken
  because rustc does not coerce in it, nor a `let` ascribed with the attempt's
  type, which misses an early `return` (`.claude/decisions/resilience-policy.md`,
  2026-11-22).
- **Canary** — none in the suite: the anchor holds whether or not an async
  closure coerces. The Ticket's program is the check, built as Seen says, in
  both editions, each time `rust-toolchain.toml`'s pin moves. Each of its four
  async closures is a case — declared or deduced, coerced at the tail or at an
  early `return` — and a case is fixed once its `E0308` lines are gone; the
  issue is, once the program builds. *When fixed* needs `tail` and `early`, the
  two closures declaring their return type.
- **Upstream** — not filed. No issue was seen; a search of rust-lang/rust's
  issues on async closures and coercion comes before filing.
- **When fixed** — the expansion may hand the body to its run as an async
  closure declaring `<Answer as ResilientAnswer>::Attempt`, and drop the anchor
  and `__private::attempt`; the replayed closure under `idempotent`, the `Send`
  of a run and every resilience snapshot are then checked again, and the compile
  cost measured. About a day.
- **Ticket** —

  > **An async closure's body is not coerced to its return type**
  >
  > ```rust
  > use std::future::Future;
  >
  > struct Hub(String);
  >
  > impl Hub {
  >     async fn departure(&self, early: bool) -> Result<&str, ()> {
  >         if early {
  >             return Ok(&self.0); // compiles
  >         }
  >         Ok(&self.0) // compiles
  >     }
  > }
  >
  > fn run<'a, F, Fut>(call: F) -> Fut
  > where
  >     F: FnOnce() -> Fut,
  >     Fut: Future<Output = Result<&'a str, ()>>,
  > {
  >     call()
  > }
  >
  > fn main() {
  >     let hub = Hub("the Lyon hub".to_owned());
  >     let sync = |early: bool| -> Result<&str, ()> {
  >         if early {
  >             return Ok(&hub.0); // compiles
  >         }
  >         Ok(&hub.0) // compiles
  >     };
  >     let tail = async || -> Result<&str, ()> { Ok(&hub.0) }; // E0308
  >     let early = async || -> Result<&str, ()> { // E0308
  >         if hub.0.is_empty() {
  >             return Ok(&hub.0);
  >         }
  >         Ok("elsewhere") // E0308: expected `&String`
  >     };
  >     let deduced_tail = run(async || Ok(&hub.0)); // E0308
  >     let deduced_early = run(async || { // E0308
  >         if hub.0.is_empty() {
  >             return Ok(&hub.0);
  >         }
  >         Ok("elsewhere") // E0308: expected `&String`
  >     });
  >     drop((hub.departure(true), sync(true), tail(), early()));
  >     drop((deduced_tail, deduced_early));
  > }
  > ```
  >
  > rustc 1.100.0 coerces `&String` into `&str` at the tail and at an early
  > `return` of an `async fn` and of a closure declaring its return type, but
  > not of an async closure declaring the same type, nor of one whose return
  > type is deduced from an `FnOnce() -> Fut, Fut: Future<Output = …>` bound:
  > the six `E0308` the comments mark, in editions 2021 and 2024. Each async
  > closure gets "expected `async` closure body … (`str`) found `async` closure
  > body … (`String`)", and the tail after an early `return` "expected
  > `&String`, found `&str`". With every coercion written out,
  > `Ok(hub.0.as_str())`, the program builds. The body seems checked as an
  > async block whose output is inferred, then equated with the declared type.
  > Is the declared return type meant to be the expected type of the body, as
  > an `async fn`'s is?

## A bound failing on a lifetime alone is worded by no `#[diagnostic]` attribute

- **Seen** — 1.100.0 (`0ef1bc386 2026-11-09`), edition 2024: the Ticket's
  program, saved as `ticket.rs` and built alone with
  `rustc +1.100 --edition 2024 ticket.rs`. Its trait carries
  `#[diagnostic::on_unimplemented]`, and its one impl, for `Result<(), E>`,
  holds for the error the program passes but for `E: 'static`, the error
  borrowing a local: rustc answers `E0521` and `E0597`, borrowck's, and the
  trait's message appears nowhere.
- **Effect** — a refusal nestrs words for every other misuse reaches a developer
  in rustc's words when only a lifetime fails: a tick answering an error that
  borrows its host is `E0716` at `#[scheduled]` for an `async` tick, naming the
  expansion's async block, and `E0515` for a `fn`, where an error converting into
  no `anyhow::Error` is the framework's sentence.
- **Our side** — a documented limit: `#[scheduled]`'s rustdoc
  (`crates/nest-rs-schedule-macros/src/lib.rs`), `docs/src/content/docs/schedule/index.mdx`
  and `.claude/decisions/resilience-policy.md`, 2026-11-22, *Limits, named*;
  both errors are pinned by `nest-rs-macro-hygiene`'s snapshot
  `schedule/scheduled_returns_a_result_whose_error_converts`.
- **Canary** — none in the suite: the snapshot moves only once `TickAnswer`
  carries what a fix adds. The Ticket's program is the check, built as Seen says,
  each time `rust-toolchain.toml`'s pin moves.
- **Upstream** — not filed. A search of rust-lang/rust's issues on the
  `diagnostic` namespace and region errors comes before filing.
- **When fixed** — `TickAnswer` words the borrowing error as it words the
  others, and the snapshot, the rustdoc, the page and the decision say the
  framework's sentence. An hour.
- **Ticket** —

  > **`#[diagnostic::on_unimplemented]` is not consulted when an impl fails on a lifetime alone**
  >
  > ```rust
  > use std::fmt;
  >
  > #[diagnostic::on_unimplemented(
  >     message = "a tick returns `Result<(), E>` whose `E` owns what it holds, and this one returns `{Self}`",
  >     label = "an error borrowing what it reports"
  > )]
  > trait TickAnswer {}
  >
  > impl<E: std::error::Error + Send + Sync + 'static> TickAnswer for Result<(), E> {}
  >
  > fn answered<A: TickAnswer>(_: A) {}
  >
  > struct Tasks;
  >
  > #[derive(Debug)]
  > struct Refused<'a>(&'a str);
  >
  > impl fmt::Display for Refused<'_> {
  >     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
  >         f.write_str(self.0)
  >     }
  > }
  >
  > impl std::error::Error for Refused<'_> {}
  >
  > impl Tasks {
  >     fn trim<'a>(&'a self, why: &'a str) -> Result<(), Refused<'a>> {
  >         Err(Refused(why))
  >     }
  > }
  >
  > fn tick(tasks: &Tasks) {
  >     let why = String::from("the prune was refused");
  >     answered(tasks.trim(&why));
  > }
  >
  > fn main() {
  >     tick(&Tasks);
  > }
  > ```
  >
  > rustc 1.100.0 answers `E0521` ("borrowed data escapes outside of function")
  > and `E0597` (`why` does not live long enough), pointing at
  > `answered(tasks.trim(&why))`; the impl fails on its `'static` alone, and the
  > trait's `on_unimplemented` message, which says what to write, is not shown. A
  > library whose macro expansion holds such a bound can word every other
  > failure of it and not this one, which the user reads as an error in code they
  > did not write. Could `on_unimplemented` be consulted, or a sibling attribute
  > offered, when an impl is rejected on a region bound it requires?

## An `async` fn or block awaiting a future it was handed holds it twice

- **Seen** — 1.100.0 (`0ef1bc386 2026-11-09`), debug and release alike:
  `nest-rs-resilience`'s
  `a_decorated_methods_future_holds_its_body_and_each_argument_once` and
  `a_runs_future_holds_its_calls_closure_and_each_attempts_future_once`, run
  against the run path as `async` fns and blocks awaiting what they were handed:
  a method holding 16 KiB across an `.await` was a 33,328 B future decorated
  against 16,416 B undecorated, and a `run` whose closure moved 16 KiB in, 82,384 B
  for a 16,408 B call. An `async` fn's argument is held twice too, as an upvar
  and as the local it is moved into: an undecorated method taking 16 KiB by value
  is a 32,800 B future.
- **Effect** — each layer of `async` code a call goes through holds it again, so a
  future several layers deep outgrows what its body holds, and overflows a
  worker's stack at a fraction of the state its body runs with alone.
- **Our side** — the futures holding a call are written by hand, polled in place:
  `contain` and `within` (`crates/nest-rs-core/src/panic.rs`,
  `crates/nest-rs-core/src/net.rs`), a tick's `answered`
  (`crates/nest-rs-schedule/src/answer.rs`), and a resilience run's
  (`crates/nest-rs-resilience/src/policy.rs`, `src/resilient.rs`), each held by a
  size test.
- **Canary** — none in the suite: the size tests hold our side whether or not
  rustc fixes the layout.
- **Upstream** — rust-lang/rust#62958.
- **When fixed** — the hand-written futures may become `async` fns again, each
  size test and the 2 MiB-worker tests beside them checked again. A day.
- **Ticket** — filed as rust-lang/rust#62958; a comment there may add the
  measurements under Seen.
