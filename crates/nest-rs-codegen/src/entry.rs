//! `#[main]`'s grammar: an `async fn`, and no arguments.
//!
//! The decorator builds the runtime the function's body runs on and tears it
//! down within the shutdown budget, so there is nothing for an argument to
//! choose. The runtime is tokio's multi-threaded one with every driver enabled —
//! what an app's transports need — and its size is the deployment's, through
//! `TOKIO_WORKER_THREADS`, rather than the source's. Both refusals name that
//! fact rather than calling an argument unknown, because a developer arriving
//! from `#[tokio::main]` writes `flavor` or `worker_threads` expecting them to
//! mean what they meant there.

use proc_macro2::{Span, TokenStream};

/// The decorator's name, as its refusals print it.
pub const ENTRY: &str = "main";

/// Refuse any argument: the runtime is not the source's to choose.
pub fn entry_takes_no_arguments(args: &TokenStream) -> Option<syn::Error> {
    use quote::ToTokens as _;
    let first = args.clone().into_iter().next()?;
    Some(syn::Error::new_spanned(
        first.into_token_stream(),
        format!(
            "#[{ENTRY}] takes no arguments: it builds tokio's multi-threaded runtime with every \
             driver enabled, sized by `TOKIO_WORKER_THREADS`, and tears it down within the \
             shutdown budget — there is nothing left for an argument to choose",
        ),
    ))
}

/// Refuse an item that is not an `async fn`: the decorator runs the function's
/// body on the runtime it builds, and only an `async` body has one to run.
pub fn entry_needs_an_async_fn(span: Span) -> syn::Error {
    syn::Error::new(
        span,
        format!(
            "#[{ENTRY}] goes on an `async fn` — the binary's `main` — whose body it runs on the \
             runtime it builds; write `#[nest_rs::main] async fn main()`",
        ),
    )
}
