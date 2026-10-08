//! `#[main]`'s grammar: an `async fn`, and no arguments.

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

/// Refuse an item that is not an `async fn`.
pub fn entry_needs_an_async_fn(span: Span) -> syn::Error {
    syn::Error::new(
        span,
        format!(
            "#[{ENTRY}] goes on an `async fn` — the binary's `main` — whose body it runs on the \
             runtime it builds; write `#[nest_rs::main] async fn main()`",
        ),
    )
}
