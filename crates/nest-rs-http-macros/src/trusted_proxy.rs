//! `trusted_proxy!` — a literal read as the program compiles, the entry it
//! names checked by the constant `nest_rs_http::TrustedProxy` evaluates.

use std::net::IpAddr;

use nest_rs_codegen::__private::trusted_proxy_entry;
use proc_macro2::{Literal, TokenStream};
use quote::{quote, quote_spanned};
use syn::LitStr;

/// The block `trusted_proxy!(input)` expands to.
pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let literal = match syn::parse2::<LitStr>(input) {
        Ok(literal) => literal,
        Err(error) => {
            return syn::Error::new(
                error.span(),
                "`trusted_proxy!` takes a string literal, `trusted_proxy!(\"10.0.0.0/8\")`; an \
                 entry read at run time is `.parse()?`",
            )
            .to_compile_error();
        }
    };
    let parsed = match trusted_proxy_entry(&literal.value()) {
        Some((address, prefix)) => {
            let address = address_tokens(address);
            let prefix = match prefix {
                Some(prefix) => quote!(::core::option::Option::Some(#prefix)),
                None => quote!(::core::option::Option::None),
            };
            quote!(::core::option::Option::Some((#address, #prefix)))
        }
        None => quote!(::core::option::Option::None),
    };
    // A constant item, not an inline `const { }`: `cargo check` evaluates it, so
    // the refusal reaches an editor and a check, not only a build.
    quote_spanned! {literal.span()=>
        {
            const TRUSTED_PROXY: ::nest_rs_http::TrustedProxy =
                ::nest_rs_http::__private::trusted_proxy_literal(#parsed);
            TRUSTED_PROXY
        }
    }
}

/// `address` as the expression that builds it.
fn address_tokens(address: IpAddr) -> TokenStream {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            quote!(::core::net::IpAddr::V4(::core::net::Ipv4Addr::new(#a, #b, #c, #d)))
        }
        IpAddr::V6(v6) => {
            let bits = Literal::u128_suffixed(v6.to_bits());
            quote!(::core::net::IpAddr::V6(::core::net::Ipv6Addr::from_bits(#bits)))
        }
    }
}
