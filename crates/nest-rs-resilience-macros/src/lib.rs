//! The `#[resilient]` decorator, re-exported by `nest-rs-resilience`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod args;
mod expansion;
mod method;
mod resilient;
#[cfg(test)]
mod testing;

/// Runs an `async fn` method's body under the policy a field of `self` holds:
/// `#[resilient(field)]` sends the call once, `#[resilient(field, idempotent)]`
/// again after an `Err` or a timeout, and the keys after the field say how an
/// answer counts, which method answers in place of the error a run ends on,
/// and under what name the body reads its attempt's token.
///
/// The field holds an `Arc<ResiliencePolicy<I>>`, which a provider takes
/// under `#[inject]`, so the boot refuses an app whose modules do not import
/// the policy's `ResilienceModule<I>`. It is read by its name, as `self.field`:
/// a tuple struct's position is refused, and so is a call — the policy is held
/// in a named field, never answered by a method, which named in the field's
/// place is rustc's `E0615`, its help writing that call. The list's first entry
/// is that field, whatever its name, one of a key that takes a value included
/// (`#[resilient(fallback)]`). A field named `idempotent`, the one key written
/// alone, is told from it by the key after it,
/// `#[resilient(idempotent, idempotent)]`, and is renamed where the method is
/// not; one named as a setting of the policy's is renamed, as its refusal says.
///
/// The method answers `Result<T, ResilienceError<E>>` — read by its type, so an
/// alias reads as the literal does, and where its signature is, whatever items
/// the body declares — and its body is written as if it answered `Result<T, E>`:
/// `?`, `Ok(..)`, `Err(e)` and an early `return` answer one attempt, each
/// answer coerced as the undecorated method's would be — a `&String` into the
/// `&str` it returns, a box into its `Box<dyn …>` — and each `?` converting its
/// error into `E`. That `E` is a `std::error::Error` owning what it says
/// (`'static`), run once or again: its chain, read as one, is the `error` of
/// the `warn` the breaker files when that failure opens it and of the line
/// each retry files, and a retry reads it for the `Retry-After` of a `Problem`
/// it carries. An error that is none — `anyhow::Error`, a `Box<dyn Error>` —
/// is held in a type of the app's own that is, which each `?` converts into.
/// One borrowing an argument, or generic over a type that is not `'static`, is
/// rustc's lifetime error at the answer — `E0310` for the generic one, whose
/// help adds the bound — and owns the value instead. `T` may hold an
/// `impl Trait` — `Result<impl Stream<Item = u32>, ResilienceError<E>>` on a
/// `#[subscription]` — whose hidden type rustc infers from the body, as for
/// the undecorated method; an answer that is an `impl Trait` whole names no
/// `T` nor `E`, and is refused at it.
///
/// - `idempotent` — the call is sent again after an `Err` or a timeout, up to
///   its retries. Only the developer declares it: sending again a call that is
///   not idempotent would charge a card twice. Each attempt reads the
///   arguments again, so the method binds none `mut` and takes no `&mut` (read
///   by spelling: a type alias hiding a `&mut` is not seen), and clones what an
///   attempt consumes — rustc answers a consumed argument with `E0507`, whose
///   help shows the clone. Every attempt borrows the arguments across its
///   waits, so the method's future is `Send` only where each argument taken by
///   value is `Sync`; run once under no `fallback` and no `record_if` naming
///   one, the method moves them, and its future is `Send` as the undecorated
///   method's is.
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
///   sent again by an `idempotent` method whose `retry_if` takes it. Run once
///   under a predicate naming an argument — read by its tokens, a macro's
///   arguments and a format string's captures included — the one attempt
///   borrows each argument its body only reads, so the predicate reads it
///   once that attempt answered, and the method's future is `Send` only where
///   such an argument, taken by value, is `Sync`; under one naming none, a
///   function's path or a closure over the error alone, the attempt owns
///   them, as with no key. An argument the body consumes or changes, a
///   predicate naming it does not read too: its tokens say which argument it
///   names, never how it reads it, so the refusal is rustc's borrow error at
///   the key, one code per case. Consumed,
///   it is `E0505`, and the body consumes a clone. Bound `mut` and changed, it
///   is `E0502`, and the method binds it without `mut` and changes a copy
///   inside the body, the predicate reading the argument as the method was
///   called with it. A `&mut` the body writes through is `E0500`, where a copy
///   would keep the change from the caller: the predicate reads the error and
///   the other arguments, or the method takes what the predicate needs as an
///   argument of its own.
/// - `fallback = method` — the method of `Self`, named alone, answering in
///   place of the error the run ends on: the call's own, from its last attempt,
///   or the policy's refusal — a timeout, an open breaker, a full bulkhead, a
///   reached rate limit. It is an `async fn` taking the decorated method's
///   arguments, in order, then that error by value, and answering what the
///   method answers:
///   `async fn stale(&self, zone: &str, error: ResilienceError<E>) -> Result<T, ResilienceError<E>>`.
///   An `Err` it answers is the method's. It runs once the run is over, under
///   no policy: no timeout, no retry, counted in no breaker. It is called as
///   a method, so it takes `&self` or `self: &Arc<Self>`: a function of `Self`
///   taking neither, and a field holding one, are rustc's `E0599` at the
///   fallback's name, whose help writes a call or parentheses the key
///   refuses. Its type parameters are fixed by what it is handed and what it
///   answers: one they leave open is rustc's `E0283` at the fallback's name,
///   whose help writes a type argument the key refuses too — the fallback's
///   signature fixes it, or drops it. Every attempt borrows the arguments it
///   is handed next, as under `idempotent` and with the same bound on a `Send`
///   future: the method binds each to a plain name, none `mut`, takes no
///   `&mut`, and clones what an attempt consumes, which rustc answers with
///   `E0382` at the fallback's name, the key marked as where the argument
///   moved. An argument handed on is read, so one the body reads nowhere takes
///   no leading `_`, which `clippy::used_underscore_binding` says at the key. A handler taking an extractor apart in its signature
///   (`Path(id): Path<u32>`) names it instead (`id: Path<u32>`) and takes it
///   apart in its body, and one binding by reference (`ref id`) binds by
///   value and borrows there. A parameter under a
///   `#[cfg]`, plain or inside a `#[cfg_attr]`, is handed in the builds that
///   compile it, to a fallback declaring it under the same condition. The
///   fallback's future is held as an attempt's is, in place in a release
///   build and boxed in a debug build.
/// - `fallback_if = F` — which errors the fallback answers for: a
///   `Fn(&ResilienceError<E>) -> bool`. Left out, every one, the policy's
///   refusals included. An error it refuses is the method's answer. It is
///   asked before the fallback is handed the arguments, so it reads one the
///   fallback then takes by value. Beside `fallback` alone.
/// - `cancel = name` — the body reads its attempt's `CancellationToken` under
///   `name`: each attempt's own, cancelled when the policy gives that attempt
///   up at its timeout or its unit's deadline, so work the body handed to
///   another task stops with it. The name is a local of the body, and no
///   argument's: one an argument already has is refused, and so is an
///   argument bound by a macro's pattern (`parcel!(weight): u32`) beside
///   `cancel`, since no attribute reads the names such a pattern binds.
///
/// A predicate is an expression of the method: a closure reads `self` and the
/// arguments by reference, as the `idempotent` method's attempts, the one
/// attempt of a method run once under a `record_if` naming one, and the
/// attempts of one naming a `fallback` do. It takes the one value it judges and answers at
/// once, so a closure of another arity and an `async` one are refused where
/// written, and a method of the host, whose path takes `self` first, is
/// called from a closure: `|error| self.is_transient(error)`. Named as
/// `self.is_transient`, it is rustc's `E0615`, whose help writes a call no
/// predicate is. A timeout is no
/// `E`, so no predicate reads it: it is always a failure in the breaker, and
/// always sent again by an `idempotent` method. `retry_if`, `retry_ok_if` and
/// `record_if` judge an attempt's answer, so one that panics fails its attempt
/// as a panic in the body does: one failure in the breaker, the savepoint it
/// ran in rolled back, and the panic unwinds from the method. `fallback_if`
/// and the fallback are asked once the run is over, outside every attempt: a
/// panic in either unwinds from the method and fails none — the run stays
/// counted as it ended, the breaker's window and the bulkhead as it left them,
/// its last attempt's savepoint already rolled back — and files no line of its
/// own, the fallback's coming after the line and the count saying it answers.
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
/// settles; the method's future dropped mid-attempt, once that attempt's
/// savepoint was answered, leaves what holds that savepoint unable to commit,
/// as an atomic block dropped does: its unit — or, nested in another attempt's
/// savepoint or in an atomic block, that savepoint or that block alone, the
/// unit going on to commit what it wrote beside it. Dropped sooner, its unit
/// waiting for a connection, its `BEGIN` or that answer, it leaves the unit as
/// it was; and a savepoint the database cannot settle ends the method on
/// `ResilienceError::Unsettled`, attempted no further and handed to no
/// fallback. A method run once with no fallback opens none, nor does any
/// outside a transaction — no unit of work, or one on the pool: an `Err` there
/// leaves what the body wrote, as it does with no policy.
///
/// Everything else on the method stays as written: its visibility, generics,
/// lifetimes, docs, `#[cfg]`s and other attributes, and its body's tokens and
/// spans, the braces it is written in included: what its tail expression
/// leaves drops as the edition of the crate writing it says, ahead of the
/// body's locals in edition 2024 and after them before it. Its arguments drop
/// as the undecorated method's do, last declared first, however its run ends:
/// answered, failed, panicked, dropped, or refused unsent. Run once under no
/// `fallback` and no `record_if` naming an argument, the one attempt owns the
/// arguments its body names, so what the body does not name — an argument it reads nowhere, the
/// part of a pattern it leaves unbound — stays in the method's frame and
/// drops once the attempt has, and an argument the body names under a leading
/// `_` drops with the attempt, after the others. Run once under a `record_if`
/// naming an argument and no `fallback`, an argument the body consumes is the attempt's own: one
/// it has not consumed when the attempt ends — failed sooner, cut, or refused
/// unsent — drops with it, ahead of the arguments the body only reads and in
/// the order the body first names them, which a body opening on
/// `let _ = (&out, &held);` sets last declared first. It takes `&self` or
/// `self: &Arc<Self>`, on any host holding its policy in a named field — a
/// provider, a struct built by hand, a native `async fn` in a trait impl, a
/// handler of every impl half, a scheduled tick included. A trait's own
/// method carries none: its declaration is refused
/// saying to decorate each impl's, and a default body, whose `Self` has no
/// field, is rustc's `E0609` at the field named. A method carries it once: a
/// second, under whatever name it is imported, is refused. A refused method is
/// its refusals: they stand in its body's place, so nothing the body would
/// make rustc say joins them, and its own errors come once they are fixed.
///
/// On an `#[sse]` route or a `#[subscription]`, the policy runs the call
/// opening the stream, never the items it yields later. A WebSocket connection
/// hook answers `()`, so it carries none and calls a decorated method instead.
/// A GraphQL field writes its `Result<T, ResilienceError<E>>` out:
/// `#[operations]` reads a field's `Result` by its spelling, and a `Result`
/// under another name whose error async-graphql cannot answer is refused at its
/// return type in one sentence. `#[operations]` leaves this attribute on the
/// method as it leaves every attribute but its own, under any name and in any
/// `#[cfg_attr]`. A method under `#[async_trait]` is a `fn` by the time this
/// attribute reads it, and is refused saying so.
///
/// `#[tracing::instrument]` goes below it, and one span covers the whole run —
/// every attempt, the waits between them, what the policy refuses unsent and
/// the fallback — and its `err` and `ret` record what the method answers.
/// Above, tracing expands first and wraps the body in a future answering the
/// method's type, where an attempt answers `Result<T, E>`: the method is
/// refused at the instrument, saying to write it below. The wrapper is told by
/// the prefix every name tracing-attributes binds shares, `__tracing_`, a
/// courtesy for the sentence; a release binding none under it leaves rustc's
/// `E0308` at the body's answers. A span per attempt is that of a method the
/// body calls, instrumented itself. The token `cancel` names is no argument of
/// the method, so no span records it. The future tracing builds owns the
/// arguments and drops them in the order its tokens first name them — below
/// the decorator, the order the expansion does, last declared first when the
/// method is run once. On a GraphQL operation the instrument is the method's,
/// as on a route, and its `err` and `ret` record what the method answers.
///
/// A run of a decorated method names it: the `debug` lines a retry made or
/// given up, a savepoint left unsettled, a full bulkhead or a reached rate
/// limit, and a fallback answering files carry its `host` and `method`.
#[proc_macro_attribute]
pub fn resilient(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(resilient::resilient(args.into(), item.into())).into()
}
