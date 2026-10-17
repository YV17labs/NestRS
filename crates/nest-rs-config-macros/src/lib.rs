//! The `#[config]` decorator, re-exported by `nest-rs-config`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod config;

/// The `#[config(namespace = "…")]` decorator: implements `Namespaced` and files
/// the namespace with the link-time registry (`nest_rs_config::unclaimed`); a
/// second struct declaring the same namespace is refused at the read of either.
///
/// Must sit **above** the derives so it sees them intact. `namespace` must be
/// a non-empty lowercase string. It carries the `Validate` derive itself;
/// `validate = "manual"` suppresses it for a config writing `impl Validate` by hand.
#[proc_macro_attribute]
pub fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(config::config(args, input).into()).into()
}
