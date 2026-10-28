//! `#[subject]`, re-exported by `nest-rs-authz`.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod subject;

/// Declares a unit struct an authorization subject no table holds: a
/// capability a rule grants whole and `#[authorize(Action, Subject)]` gates on
/// every edge, arming no mask.
///
/// Takes no argument; refuses a struct with fields or generics, and any other
/// item. Emits the struct unchanged, its `Subject` impl, and the hidden impl
/// that tells each edge's posture the subject has no rows to mask.
#[proc_macro_attribute]
pub fn subject(args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(subject::subject(args.into(), item.into())).into()
}
