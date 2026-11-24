//! The `#[resilient]` decorator, re-exported by `nest-rs-resilience`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod instrumented;
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
/// early `return` answer one attempt, each answer coerced as the undecorated
/// method's would be — a `&String` into the `&str` it returns, a box into its
/// `Box<dyn …>` — and each `?` converting its error into `E`.
///
/// - `idempotent` — the call is sent again after a failure or a timeout, up to
///   its retries. Only the developer declares it: sending again a call that is
///   not idempotent would charge a card twice. Each attempt reads the
///   arguments again, so the method binds none `mut` and takes no `&mut` (read
///   by spelling: a type alias hiding a `&mut` is not seen), and clones what an
///   attempt consumes — rustc answers a consumed argument with `E0507`, whose
///   help shows the clone. Every attempt borrows the arguments across its
///   waits, so the method's future is `Send` only where each argument taken by
///   value is `Sync`; run once, the method moves them, and its future is `Send`
///   as the undecorated method's is. Its error is a `std::error::Error`, whose
///   chain a retry reads for a `Retry-After`.
/// - `timeout = "2s"` — how long each attempt of this method waits, in place
///   of the policy's, from `"1ms"` to `"10m"`. The unit's deadline still ends
///   every attempt.
/// - `retries = 3` — this method's retries, in place of the policy's, from 1
///   to 10, beside `idempotent` alone: a call run once is never sent again.
///
/// Every other setting is the policy's, shared by every caller of the
/// dependency — the backoff's, the circuit breaker's, the bulkhead's and the
/// rate limit's — so a method pins none: a key naming one, before the field
/// or after it, is refused with the variable and the `ResilienceConfig` field
/// that set it.
///
/// Everything else on the method stays as written: its visibility, generics,
/// lifetimes, docs, `#[cfg]`s and other attributes, and its body's tokens and
/// spans. It takes `&self` or `self: &Arc<Self>`, on any host — a provider, a
/// struct built by hand, a native `async fn` in a trait impl, a handler of
/// every impl half, a scheduled tick included. On an `#[sse]` route or a
/// `#[subscription]`, the policy runs the call opening the stream, never the
/// items it yields later. A WebSocket connection hook answers `()`, so it
/// carries none and calls a decorated method instead. A GraphQL field writes
/// its `Result<T, ResilienceError<E>>` out: `#[operations]` reads a field's
/// `Result` by its spelling, and refuses any other answer of a decorated one. It
/// keeps this attribute, read by its last segment, on the method — written
/// plain, or as an entry of a `#[cfg_attr]`, kept under its condition — and
/// hands the method's other attributes and entries to the field it generates,
/// in a build with `graphql` and `resilience` both, whichever crate of it
/// enables them, the umbrella's `resilience` enabling `nest-rs-graphql`'s. So
/// the decorator is imported as `resilient` there: under another name it is
/// handed to the field, and refused saying so. An app's own attribute of this
/// name stays on the method too, where it fails to compile or applies to the
/// method alone, so it is imported under another name; without `resilience`
/// one of this name goes on the field, as 7.0.0 placed it. A method under
/// `#[async_trait]` is a `fn` by the time this attribute reads it, and is
/// refused saying so.
///
/// `#[tracing::instrument]` goes above it or below, and the order says what
/// the span covers. Below `#[resilient]`, one span covers the whole run —
/// every attempt, the waits between them and what the policy refuses unsent —
/// and its `err` and `ret` record what the method answers. Above, tracing
/// expands first: each attempt opens a span of its own, and `err` and `ret`
/// record what that attempt answered, the dependency's own error. Above, the
/// decorator reads the body as tracing-attributes 0.1 wraps it, and refuses one
/// wrapped otherwise saying to write `#[resilient]` above.
///
/// A run of a decorated method names it: the `debug` lines a retry, a full
/// bulkhead or a reached rate limit files carry its `host` and `method`.
#[proc_macro_attribute]
pub fn resilient(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(resilient::resilient(args.into(), item.into())).into()
}
