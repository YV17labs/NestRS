//! `#[scheduled]` — orchestrator on a provider's `impl` block. Walks the
//! methods, finds those tagged with `#[cron(...)]` / `#[every("...")]` /
//! `#[after("...")]`, strips the attribute, and submits one
//! `ScheduledMethod` per method to the link-time inventory. The methods stay
//! on the impl block unchanged so they remain regular `async fn` callable
//! from anywhere.
//!
//! Discoverable is NOT emitted here — the provider's own `#[injectable]` owns
//! it. Inventory is exactly the seam `#[hooks]` uses for lifecycle methods,
//! for the same reason.

use std::str::FromStr;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, ImplItem, ItemImpl, Lit, LitStr, MetaNameValue, Token, Type,
    parse_macro_input,
};

pub(crate) fn scheduled(_args: TokenStream, input: TokenStream) -> TokenStream {
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

        let trigger_idx = method
            .attrs
            .iter()
            .position(|attr| is_trigger_attr(attr.path()));
        let Some(idx) = trigger_idx else { continue };
        let trigger_attr = method.attrs.remove(idx);

        // A second trigger attribute on the same method is a per-method
        // mutual-exclusion violation — surface it crisply at compile.
        if let Some(extra) = method
            .attrs
            .iter()
            .find(|attr| is_trigger_attr(attr.path()))
        {
            return syn::Error::new(
                extra.span(),
                "a scheduled method takes exactly one trigger — \
                 `#[cron(...)]`, `#[every(\"...\")]`, or `#[after(\"...\")]`",
            )
            .to_compile_error()
            .into();
        }

        let trigger_tokens = match parse_trigger(&trigger_attr) {
            Ok(tokens) => tokens,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.to_string();

        submissions.push(quote! {
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_schedule::ScheduledMethod {
                    provider: #provider_name,
                    method: #method_name,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    trigger: #trigger_tokens,
                    run: |__container| ::std::boxed::Box::pin(async move {
                        let __provider = ::nest_rs_core::Container::get::<#self_ty>(__container)
                            .expect(::std::concat!(
                                "scheduled provider `", #provider_name,
                                "` is not registered — add it to a reachable module's \
                                 `providers = [...]`",
                            ));
                        <#self_ty>::#method_ident(&__provider).await
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

fn is_trigger_attr(path: &syn::Path) -> bool {
    path.is_ident("cron") || path.is_ident("every") || path.is_ident("after")
}

fn parse_trigger(attr: &Attribute) -> syn::Result<TokenStream2> {
    let key = attr
        .path()
        .get_ident()
        .map(ToString::to_string)
        .unwrap_or_default();
    match key.as_str() {
        "every" => {
            let lit: LitStr = attr.parse_args()?;
            let ms = period_millis(&lit)?;
            Ok(quote! {
                ::nest_rs_schedule::Trigger::Interval(
                    ::std::time::Duration::from_millis(#ms)
                )
            })
        }
        "after" => {
            let lit: LitStr = attr.parse_args()?;
            let ms = period_millis(&lit)?;
            Ok(quote! {
                ::nest_rs_schedule::Trigger::Timeout(
                    ::std::time::Duration::from_millis(#ms)
                )
            })
        }
        "cron" => parse_cron(attr),
        _ => unreachable!("is_trigger_attr filtered the attribute set"),
    }
}

fn parse_cron(attr: &Attribute) -> syn::Result<TokenStream2> {
    let tokens = attr
        .meta
        .require_list()
        .map_err(|_| {
            syn::Error::new(
                attr.span(),
                "#[cron] expects `#[cron(\"...\")]`, \
                 `#[cron(CronExpression::EVERY_MINUTE)]`, or \
                 `#[cron(\"...\", tz = \"Europe/Paris\")]`",
            )
        })?
        .tokens
        .clone();

    let parser = |stream: syn::parse::ParseStream<'_>| -> syn::Result<(Expr, Option<LitStr>)> {
        let expr: Expr = stream.parse()?;
        let mut tz: Option<LitStr> = None;
        if stream.peek(Token![,]) {
            stream.parse::<Token![,]>()?;
            // Allow trailing comma.
            if !stream.is_empty() {
                let metas: Punctuated<MetaNameValue, Token![,]> =
                    Punctuated::parse_terminated(stream)?;
                for meta in metas {
                    let name = meta
                        .path
                        .get_ident()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    if name == "tz" {
                        tz = Some(as_str_lit(&meta.value, "tz")?);
                    } else {
                        return Err(syn::Error::new_spanned(
                            &meta.path,
                            format!("unknown #[cron] argument `{name}`; expected `tz`"),
                        ));
                    }
                }
            }
        }
        Ok((expr, tz))
    };
    let (expr, tz) = parser.parse2(tokens)?;

    // Literal cron expressions validate now; `CronExpression::X` paths wait
    // for boot (the `Scheduler::configure` call).
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = &expr
        && let Err(e) = croner::Cron::from_str(&s.value())
    {
        return Err(syn::Error::new(
            s.span(),
            format!("invalid cron expression: {e}"),
        ));
    }
    let tz_tokens = match tz {
        Some(lit) => quote! { ::std::option::Option::Some(#lit) },
        None => quote! { ::std::option::Option::None },
    };
    Ok(quote! {
        ::nest_rs_schedule::Trigger::Cron { expr: #expr, tz: #tz_tokens }
    })
}

fn as_str_lit(value: &Expr, key: &str) -> syn::Result<LitStr> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = value
    {
        Ok(s.clone())
    } else {
        Err(syn::Error::new_spanned(
            value,
            format!("#[cron] `{key}` must be a string literal, e.g. `{key} = \"...\"`"),
        ))
    }
}

fn period_millis(lit: &LitStr) -> syn::Result<u64> {
    let raw = lit.value();
    let s = raw.trim();
    let bad = || {
        syn::Error::new(
            lit.span(),
            "duration must be a positive number with an `ms`, `s`, `m`, or `h` suffix \
             (e.g. \"500ms\", \"30s\", \"5m\", \"1h\")",
        )
    };
    // `ms` before `s` so "500ms" is not mis-read as "500m".
    let (number, multiplier) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1u64)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        return Err(bad());
    };
    let value: u64 = number.trim().parse().map_err(|_| bad())?;
    if value == 0 {
        return Err(syn::Error::new(
            lit.span(),
            "duration must be greater than zero",
        ));
    }
    value
        .checked_mul(multiplier)
        .ok_or_else(|| syn::Error::new(lit.span(), "duration overflows u64 milliseconds"))
}

fn impl_self_name(self_ty: &Type) -> syn::Result<String> {
    if let Type::Path(p) = self_ty
        && let Some(seg) = p.path.segments.last()
    {
        return Ok(seg.ident.to_string());
    }
    Err(syn::Error::new_spanned(
        self_ty,
        "#[scheduled] expects an `impl` block on a named struct (e.g. `impl AudioTasks`)",
    ))
}
