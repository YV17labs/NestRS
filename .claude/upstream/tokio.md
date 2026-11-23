# tokio

## The size past which a spawned future is boxed is private

- **Seen** — 1.54.1: `runtime::BOX_FUTURE_THRESHOLD` (`src/runtime/mod.rs`)
  is `pub(crate)`, 2048 bytes in a debug build and 16384 in a release one:
  past it, `spawn` boxes a future before it becomes a task, decided per type
  by `AutoBox::SHOULD_BOX`, so a large one is not moved through the stack.
- **Effect** — a library holding a caller's closure or future inside its own
  future, and making the same trade-off, cannot read the size tokio chose: it
  copies the number, which a tokio release may change.
- **Our side** — `BOX_THRESHOLD` in `crates/nest-rs-resilience/src/call.rs`
  copies the release value: a release build's run holds its closure where it
  is up to it, and boxes it past it; a debug build boxes every closure, so the
  debug value is not copied. On a tokio bump, the constant is read again in
  tokio's source and the copy moved with it.
  `a_closure_up_to_tokios_threshold_is_held_in_place_and_one_past_it_boxed_in_a_release_build`
  (`tests/integration/policy.rs`) holds the copy on both sides of 16384, never
  tokio's value.
- **Canary** — none: no test can read a `pub(crate)` constant.
- **Upstream** — not filed.
- **When fixed** — `BOX_THRESHOLD` reads tokio's constant, and the bump's
  check goes. An hour.
- **Ticket** —

  > **Expose the size past which `spawn` boxes a future**
  >
  > `runtime::BOX_FUTURE_THRESHOLD` decides, per profile, when a spawned
  > future is boxed so that a large one is not moved through the stack. A
  > library holding a user's closure or future inside a future of its own
  > faces the same trade-off, and would make it at the size tokio does. Could
  > the constant, or a `const fn` returning it, be public, documented as
  > free to change between releases?
