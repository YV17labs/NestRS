//! The `#[resilient]` decorator, re-exported by `nest-rs-resilience`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod resilient;

/// Runs an `async fn` method's body under the policy a field of `self` holds:
/// `#[resilient(field)]` sends the call once, `#[resilient(field, idempotent)]`
/// again after a failure or a timeout.
///
/// The field is the method's own `#[inject]` of an `Arc<ResiliencePolicy<I>>`,
/// so the boot refuses an app whose modules do not import the policy's
/// `ResilienceModule<I>`. The method answers `Result<T, ResilienceError<E>>` —
/// read by its type, so an alias reads as the literal does — and its body is
/// written as if it answered `Result<T, E>`: `?`, `Ok(..)`, `Err(e)` and an
/// early `return` answer one attempt.
///
/// - `idempotent` — the call is sent again after a failure or a timeout, up to
///   its retries. Only the developer declares it: sending again a call that is
///   not idempotent would charge a card twice. Each attempt reads the
///   arguments again, so the method binds none `mut` and takes no `&mut` (read
///   by spelling: a type alias hiding a `&mut` is not seen), and clones what an
///   attempt consumes — rustc answers a consumed argument with `E0507`, whose
///   help shows the clone. Its error is a `std::error::Error`, whose chain a
///   retry reads for a `Retry-After`.
/// - `timeout = "2s"` — how long each attempt of this method waits, in place
///   of the policy's, from `"1ms"` to `"10m"`. The unit's deadline still ends
///   every attempt.
/// - `retries = 3` — this method's retries, in place of the policy's, from 1
///   to 10, beside `idempotent` alone: a call run once is never sent again.
///
/// The breaker and the bulkhead take no pin: their state is the dependency's,
/// shared by every caller of the policy.
///
/// Everything else on the method stays as written: its visibility, generics,
/// lifetimes, docs, `#[cfg]`s and other attributes, and its body's tokens and
/// spans. It takes `&self` or `self: &Arc<Self>`, on any host — a provider, a
/// struct built by hand, a native `async fn` in a trait impl, a handler of
/// every impl half but `#[scheduled]`'s, whose tick answers
/// `anyhow::Result<()>` and so calls a decorated method. A method under
/// `#[async_trait]` is a `fn` by the time this attribute reads it, and is
/// refused saying so.
///
/// A run of a decorated method names it: the `debug` lines a retry or a full
/// bulkhead files carry its `host` and `method`.
#[proc_macro_attribute]
pub fn resilient(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(resilient::resilient(args.into(), item.into())).into()
}
