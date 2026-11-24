//! The `#[resilient]` decorator, re-exported by `nest-rs-resilience`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod instrumented;
mod resilient;

/// Runs an `async fn` method's body under the policy a field of `self` holds:
/// `#[resilient(field)]` sends the call once, `#[resilient(field, idempotent)]`
/// again after a failure or a timeout, and the keys after the field say how an
/// answer counts, which method answers in place of the error a run ends on,
/// and under what name the body reads its attempt's token.
///
/// The field holds an `Arc<ResiliencePolicy<I>>`, which a provider takes
/// under `#[inject]`, so the boot refuses an app whose modules do not import
/// the policy's `ResilienceModule<I>`. The method answers
/// `Result<T, ResilienceError<E>>` — read by its type, so an alias reads as the
/// literal does — and its body is written as if it answered `Result<T, E>`:
/// `?`, `Ok(..)`, `Err(e)` and an early `return` answer one attempt, each
/// answer coerced as the undecorated method's would be — a `&String` into the
/// `&str` it returns, a box into its `Box<dyn …>` — and each `?` converting its
/// error into `E`. That `E` is a `std::error::Error`, run once or again: its
/// chain is the `error` of the `warn` the breaker files when that failure opens
/// it and of the line each retry files, and a retry reads it for the
/// `Retry-After` of a `Problem` it carries.
///
/// - `idempotent` — the call is sent again after a failure or a timeout, up to
///   its retries. Only the developer declares it: sending again a call that is
///   not idempotent would charge a card twice. Each attempt reads the
///   arguments again, so the method binds none `mut` and takes no `&mut` (read
///   by spelling: a type alias hiding a `&mut` is not seen), and clones what an
///   attempt consumes — rustc answers a consumed argument with `E0507`, whose
///   help shows the clone. Every attempt borrows the arguments across its
///   waits, so the method's future is `Send` only where each argument taken by
///   value is `Sync`; run once under no `record_if` and no `fallback`, the
///   method moves them, and its future is `Send` as the undecorated method's
///   is.
/// - `timeout = "2s"` — how long each attempt of this method waits, in place
///   of the policy's, from `"1ms"` to `"10m"`. The unit's deadline still ends
///   every attempt.
/// - `retries = 3` — this method's retries, in place of the policy's, from 1
///   to 10, beside `idempotent` alone: a call run once is never sent again.
/// - `retry_if = F` — which `Err` is sent again: a `Fn(&E) -> bool`, a
///   function's path or a closure, asked of each `Err` an attempt answers. Left
///   out, every `Err` is. An `Err` it refuses ends the run and is the method's
///   answer. Beside `idempotent` alone.
/// - `retry_ok_if = F` — which `Ok` is sent again: a `Fn(&T) -> bool`, for a
///   dependency answering "not ready yet" as a success. Left out, none is. Once
///   the retries are spent, the last `Ok` is the method's answer. Beside
///   `idempotent` alone.
/// - `record_if = F` — which `Err` counts as a failure in the circuit breaker:
///   a `Fn(&E) -> bool`. Left out, every `Err` does. An `Err` it refuses is the
///   dependency's answer — a declined card, a missing record — and counts in
///   neither direction, no failure and no success either: a closed breaker's
///   window holds nothing of it, and a probe answering it hands its turn to
///   the next call, its series as it was. While half-open, answers `record_if`
///   refuses neither close nor reopen the breaker, so it stays half-open, one
///   call at a time, until an `Ok` or a counted `Err`. It says nothing of the
///   retry, which is `retry_if`'s: an `Err` counted for no failure is still
///   sent again by an `idempotent` method whose `retry_if` takes it. Run once,
///   the one attempt borrows each argument its body only reads, so the
///   predicate reads it once that attempt answered, and the method's future is
///   `Send` only where such an argument, taken by value, is `Sync`. An
///   argument the body consumes or changes is the attempt's own: one the
///   predicate reads too is rustc's borrow error at the key, `E0505` or
///   `E0502`, and the body clones it.
/// - `fallback = method` — the method of `Self`, named alone, answering in
///   place of the error the run ends on: the call's own, from its last attempt,
///   or the policy's refusal — a timeout, an open breaker, a full bulkhead, a
///   reached rate limit. It is an `async fn` taking the decorated method's
///   arguments, in order, then that error by value, and answering what the
///   method answers:
///   `async fn stale(&self, zone: &str, error: ResilienceError<E>) -> Result<T, ResilienceError<E>>`.
///   An `Err` it answers is the method's. It runs once the run is over, under
///   no policy: no timeout, no retry, counted in no breaker. Every attempt
///   borrows the arguments it is handed next, as under `idempotent` and with
///   the same bound on a `Send` future: the method binds each to a plain name,
///   none `mut`, takes no `&mut`, and clones what an attempt consumes, which
///   rustc answers with `E0382` at the key. A handler taking an extractor
///   apart in its signature (`Path(id): Path<u32>`) names it instead
///   (`id: Path<u32>`) and takes it apart in its body.
/// - `fallback_if = F` — which errors the fallback answers for: a
///   `Fn(&ResilienceError<E>) -> bool`. Left out, every one, the policy's
///   refusals included. An error it refuses is the method's answer. It is
///   asked before the fallback is handed the arguments, so it reads one the
///   fallback then takes by value. Beside `fallback` alone.
/// - `cancel = name` — the body reads its attempt's `CancellationToken` under
///   `name`: each attempt's own, cancelled when the policy gives that attempt
///   up at its timeout or its unit's deadline, so work the body handed to
///   another task stops with it. The name is a local of the body, and no
///   argument's: one an argument already has is refused.
///
/// A predicate is an expression of the method: a closure reads `self` and the
/// arguments by reference, as the `idempotent` method's attempts, the one
/// attempt of a method run once under `record_if`, and the attempts of one
/// naming a `fallback` do. A timeout is no `E`, so no predicate reads it: it
/// is always a failure in the breaker, and always sent again by an
/// `idempotent` method. A predicate that panics fails its attempt as a panic
/// in the body does.
///
/// Every other setting is the policy's, shared by every caller of the
/// dependency — the backoff's, the circuit breaker's, the bulkhead's and the
/// rate limit's — so a method pins none: a key naming one, before the field
/// or after it, is refused with the variable and the `ResilienceConfig` field
/// that set it.
///
/// Inside a unit of work holding a transaction, each attempt of a method
/// declaring `idempotent` or a `fallback` runs in a savepoint of that
/// transaction, whatever its retries, none included: what the body writes
/// through the data layer is kept only when the method answers that attempt's
/// `Ok`, and rolled back otherwise — an `Ok` sent again included — so a failed
/// attempt leaves no write for the next to repeat, nor beside the error the
/// method answers, nor under what its fallback writes: the last attempt is
/// rolled back before the fallback runs, and before the method answers an
/// error `fallback_if` refused. Once an attempt sent a statement, another
/// statement of that unit, issued beside the method, waits until the savepoint
/// settles; the method's future dropped mid-attempt leaves its unit unable to
/// commit, as an atomic block dropped does; and a savepoint the database
/// cannot settle ends the method on `ResilienceError::Unsettled`, attempted no
/// further and handed to no fallback. A method run once with no fallback opens
/// none, nor does any outside a transaction — no unit of work, or one on the
/// pool: an `Err` there leaves what the body wrote, as it does with no policy.
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
/// wrapped otherwise saying to write `#[resilient]` above. Both orders hold
/// under every key: below, the span also covers the fallback and records what
/// it answers; above, the fallback runs outside every attempt's span. The
/// token `cancel` names is no argument of the method, so no span records it.
///
/// A run of a decorated method names it: the `debug` lines a retry made or
/// given up, a savepoint left unsettled, a full bulkhead or a reached rate
/// limit, and a fallback answering files carry its `host` and `method`.
#[proc_macro_attribute]
pub fn resilient(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(resilient::resilient(args.into(), item.into())).into()
}
