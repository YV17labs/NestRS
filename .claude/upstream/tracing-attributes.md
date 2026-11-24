# tracing-attributes

## An attribute expanded after `#[instrument]` cannot type the body it wrapped

- **Seen** — 0.1.31: `gen_block` (`src/expand.rs`) wraps an `async fn`'s body
  in `let __tracing_instrument_future = async move { … }` and opens it on a
  return never taken, `let __tracing_attr_fake_return: <the method's return
  type> = loop {};`, so the body coerces as the undecorated method's does;
  under `err` or `ret` that future's first statement awaits a second
  `async move` block holding the same two. The three names are private, and
  nothing in the expansion says where the developer's body is or what type it
  was given.
- **Effect** — an attribute written below `#[tracing::instrument]` receives
  that expansion in place of the method. One that runs the body as something
  other than the method's own answer cannot say so: under `#[resilient]`, whose
  body answers `Result<T, E>` where the method answers
  `Result<T, ResilienceError<E>>`, the body stopped compiling, three `E0308` at
  the developer's lines.
- **Our side** — `crates/nest-rs-resilience-macros/src/instrumented.rs` reads
  the expansion by those names and by position: it gives the return never taken
  the attempt's type, and drops the `move` of tracing's own blocks where each
  attempt borrows the arguments. A body carrying the names in another shape is
  one error, saying to write `#[resilient]` above `#[tracing::instrument]`
  (`resilient_under_an_instrument_it_cannot_read.stderr`). On a
  tracing-attributes bump, `gen_block` is read again against that file's `//!`.
- **Canary** — none that tells a fix; a change of shape fails
  `nest-rs-resilience`'s `instrumented` module
  (`tests/integration/resilient.rs`), which runs every flavour in both orders.
- **Upstream** — not filed.
- **When fixed** — `instrumented.rs` reads the documented marker and its
  recogniser of names goes. Half a day.
- **Ticket** —

  > **Let an attribute expanded after `#[instrument]` find and type the body**
  >
  > `#[instrument]` on an `async fn` wraps the body in a future it ascribes the
  > function's return type, through `__tracing_attr_fake_return`. An attribute
  > written below it, expanded next, receives that wrapper: one that runs the
  > body as a step of something larger — a retry, a policy — and gives it
  > another type than the function's has nothing stable to read. Could the
  > expansion mark the body and its ascribed type in a documented way, or
  > `#[instrument]` take the type to ascribe?
