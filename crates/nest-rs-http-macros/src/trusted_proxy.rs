//! `trusted_proxy!` — a literal read as the program compiles, the entry it
//! names checked by the constant `nest_rs_http::TrustedProxy` evaluates.

use std::net::IpAddr;

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
    let parsed = match entry(&literal.value()) {
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

/// The address and the prefix length `text` spells, as `TrustedProxy`'s
/// `FromStr` reads them; `None` for text that is neither form.
fn entry(text: &str) -> Option<(IpAddr, Option<u8>)> {
    let Some((network, prefix)) = text.split_once('/') else {
        return Some((text.parse().ok()?, None));
    };
    // `u8`'s parser takes a leading `+`, which no CIDR notation writes.
    let prefix = Some(prefix)
        .filter(|digits| digits.bytes().all(|b| b.is_ascii_digit()))?
        .parse()
        .ok()?;
    Some((network.parse().ok()?, Some(prefix)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_is_read_as_the_type_reads_it() {
        let v4: IpAddr = "10.42.0.0".parse().unwrap();
        assert_eq!(entry("10.42.0.0/16"), Some((v4, Some(16))));
        assert_eq!(entry("10.42.0.0"), Some((v4, None)));
        assert_eq!(
            entry("fd00::/8"),
            Some(("fd00::".parse().unwrap(), Some(8)))
        );
        for refused in [
            "",
            "proxy.example",
            "10.0.0.0/",
            "/8",
            "10.0.0.0/+8",
            "10.0.0.0/256",
            "10.0.0.0 /8",
            "[::1]",
        ] {
            assert_eq!(entry(refused), None, "{refused:?}");
        }
    }
}
