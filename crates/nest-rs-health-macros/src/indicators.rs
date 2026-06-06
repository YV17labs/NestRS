//! `#[indicators]` — orchestrator on a provider's `impl` block. Walks the
//! methods, finds those tagged with `#[liveness]` / `#[readiness]` /
//! `#[startup]`, strips the attribute, and submits one `HealthIndicator` per
//! method to the link-time inventory. The methods stay on the impl block
//! unchanged so they remain regular `async fn` callable from anywhere.
//!
//! Discoverable is NOT emitted here — the provider's own `#[injectable]` owns
//! it. Inventory is exactly the seam `#[hooks]`, `#[scheduled]`, and
//! `#[processor]` use, for the same reason.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{ImplItem, ItemImpl, ReturnType, Type, parse_macro_input};

const PROBE_ATTRS: [(&str, &str); 3] = [
    ("liveness", "Liveness"),
    ("readiness", "Readiness"),
    ("startup", "Startup"),
];

pub(crate) fn indicators(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[indicators] takes no arguments; tag methods with `#[liveness]`, \
             `#[readiness]`, or `#[startup]`",
        )
        .to_compile_error()
        .into();
    }

    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();
    let provider_name = match impl_self_name(&self_ty) {
        Ok(name) => name,
        Err(err) => return err.to_compile_error().into(),
    };

    let mut submissions: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let probe = method.attrs.iter().enumerate().find_map(|(idx, attr)| {
            PROBE_ATTRS
                .iter()
                .find(|(name, _)| attr.path().is_ident(name))
                .map(|(_, variant)| (idx, *variant))
        });
        let Some((idx, kind_variant)) = probe else {
            continue;
        };
        let attr = method.attrs.remove(idx);

        // A second probe attribute on the same method is a per-method
        // mutual-exclusion violation — surface it at compile.
        if let Some(extra) = method.attrs.iter().find(|attr| {
            PROBE_ATTRS
                .iter()
                .any(|(name, _)| attr.path().is_ident(name))
        }) {
            return syn::Error::new(
                extra.span(),
                "an indicator method takes exactly one probe — \
                 `#[liveness]`, `#[readiness]`, or `#[startup]`",
            )
            .to_compile_error()
            .into();
        }

        if method.sig.asyncness.is_none() {
            return syn::Error::new_spanned(
                &method.sig,
                "#[indicators] methods must be `async fn`",
            )
            .to_compile_error()
            .into();
        }

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.to_string();
        let kind_ident = syn::Ident::new(kind_variant, attr.span());

        // Adapt the method's return to `anyhow::Result<()>`. A bare method is
        // infallible (always `up`); a returning one must yield
        // `Result<(), E: Into<anyhow::Error>>`.
        let invoke = match &method.sig.output {
            ReturnType::Default => quote! {
                <#self_ty>::#method_ident(&__provider).await;
                ::std::result::Result::Ok(())
            },
            ReturnType::Type(..) => quote! {
                ::std::result::Result::map_err(
                    <#self_ty>::#method_ident(&__provider).await,
                    ::std::convert::Into::into,
                )
            },
        };

        submissions.push(quote! {
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_health::HealthIndicator {
                    name: #method_name,
                    kind: ::nest_rs_health::ProbeKind::#kind_ident,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    run: |__container| ::std::boxed::Box::pin(async move {
                        let __provider = ::nest_rs_core::Container::get::<#self_ty>(__container)
                            .expect(::std::concat!(
                                "indicator provider `", #provider_name,
                                "` is not registered — add it to a reachable module's \
                                 `providers = [...]`",
                            ));
                        #invoke
                    }),
                }
            }
        });
    }

    let out = quote! {
        #item
        #(#submissions)*
    };
    out.into()
}

fn impl_self_name(self_ty: &Type) -> syn::Result<String> {
    if let Type::Path(p) = self_ty
        && let Some(seg) = p.path.segments.last()
    {
        return Ok(seg.ident.to_string());
    }
    Err(syn::Error::new_spanned(
        self_ty,
        "#[indicators] expects an `impl` block on a named struct (e.g. `impl AppHealth`)",
    ))
}
