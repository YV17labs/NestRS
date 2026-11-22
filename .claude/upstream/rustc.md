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
