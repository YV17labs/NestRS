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
  the expansion by two of those names, the future's and the return's, and by
  position: it gives the return never taken the attempt's type, and drops the
  `move` of tracing's own blocks where each attempt borrows the arguments. The
  names are private, and nothing holds an app to 0.1.31: `tracing = "0.1"`
  takes any 0.1.x of tracing-attributes at the app's own `cargo update`, and
  the rule forbids pinning a patch. So what is not read whole degrades by one
  sign, the `__tracing_` prefix all three names share: a body carrying a name
  under it anywhere — bound at its top, inside one expression, or below what
  another attribute wrapped tracing's statements in — in a shape the file does
  not read is one error, saying to write `#[resilient]` above
  `#[tracing::instrument]`, the order that compiles whatever tracing writes
  (`resilient_under_an_instrument_it_cannot_read.stderr`), never a body typed
  as one no instrument wrapped. A release that dropped the prefix from every
  name would be read as no instrument, and the developer's answers reported as
  three `E0308` again: the one limit left, which only a marker upstream closes.
  On a tracing-attributes bump, `gen_block` is read again against that file's
  `//!`.
- **Canary** — none that tells a fix; a change of shape fails
  `nest-rs-resilience`'s `instrumented` module
  (`tests/integration/resilient.rs`), which runs every flavour in both orders,
  on the framework's own lockfile alone. The renamed variants an app could
  meet first are that file's unit tests.
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

## `#[instrument]` changes the order an `async fn` drops its arguments in

- **Seen** — 0.1.31: `gen_block` (`src/expand.rs`) moves an `async fn`'s body
  into `async move { … }`, whose captures drop in the order the body first names
  them (`rustc.md`), where the same function without the attribute drops its
  parameters last declared first. `nest-rs-resilience`'s
  `dropped::beside_an_instrument_the_arguments_drop_in_the_order_its_future_takes_them`
  runs the instrumented method with no policy.
- **Effect** — writing `#[tracing::instrument]` on a method taking a guard and
  what it guards can release the guard first.
- **Our side** — none for an instrumented method with no policy. Under one, a
  method run once drops its arguments in the instrument's order where the
  instrument is written above `#[resilient]`, sent or refused unsent, and last
  declared first where it is written below; the same test pins both.
- **Canary** — that test: its instrumented method drops last declared first the
  day the attribute keeps the order.
- **Upstream** — not filed.
- **When fixed** — `captured` in
  `crates/nest-rs-resilience-macros/src/resilient.rs` no longer leaves out a
  method an instrument sits above. An hour.
- **Ticket** —

  > **`#[instrument]` changes an `async fn`'s drop order**
  >
  > An `async fn` drops its parameters in the reverse of their declaration. The
  > attribute moves the body into `async move { … }`, which drops its captures
  > in the order the body first mentions them, so a guard taken by value can be
  > released before the value declared after it. Opening the block on
  > `let _ = (&last, …, &first);` restores the order for the parameters the body
  > names.
