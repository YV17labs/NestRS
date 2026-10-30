//! The check every edge's tail runs on an argument it decoded, once its guards
//! and class gate admitted the caller.

use proc_macro2::TokenStream;
use quote::quote;

/// A `Result<(), nest_rs_pipes::PipeError>` expression over `argument`, a
/// reference to a decoded argument of a concrete type: the rules the type
/// declares run — an `Option`'s when it holds a value — and a type declaring
/// none compiles to nothing. Each edge renders the error as its own.
///
/// `edge` is the edge crate's root (`::nest_rs_http`), whose `__private`
/// re-exports the pipes' probe, so a crate writing a decorator needs no
/// dependency of its own on `nest-rs-pipes`.
pub fn validate_argument(edge: &TokenStream, argument: &TokenStream) -> TokenStream {
    quote! {{
        #[allow(
            unused_imports,
            reason = "one of the three choices resolves; which depends on the argument's type"
        )]
        use #edge::__private::{DeclaredRules as _, NoRules as _, OptionalRules as _};
        (&&#edge::__private::ValidateProbe(#argument)).maybe_validate()
    }}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_reads_the_argument_through_the_pipes_probe() {
        let emitted = validate_argument(&quote!(::nest_rs_http), &quote!(&body)).to_string();
        assert!(
            emitted.contains(":: nest_rs_http :: __private :: ValidateProbe (& body)"),
            "the probe borrows the argument through the edge: {emitted}"
        );
        for choice in ["DeclaredRules", "OptionalRules", "NoRules"] {
            assert!(emitted.contains(choice), "{choice} is in scope: {emitted}");
        }
    }
}
