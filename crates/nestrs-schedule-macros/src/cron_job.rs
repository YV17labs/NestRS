//! `#[cron_job]` implementation: parse the trigger, then emit the construction +
//! `Discoverable` (attaching a `CronJobMeta`).

use std::str::FromStr;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Expr, ExprLit, ItemStruct, Lit, LitStr, MetaNameValue, Token};

use nestrs_codegen::{
    build_injectable_body, from_container_method, injected_method, InjectableBody,
};

pub(crate) fn cron_job(args: TokenStream, input: TokenStream) -> TokenStream {
    let trigger = match Trigger::parse(args.into()) {
        Ok(trigger) => trigger,
        Err(err) => return err.to_compile_error().into(),
    };

    let mut item = parse_macro_input!(input as ItemStruct);
    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let name_lit = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let injected = injected_method(&dep_keys);
    let trigger_tokens = trigger.to_tokens();

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #injected

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                builder.attach_meta::<Self, ::nestrs_schedule::CronJobMeta>(
                    ::nestrs_schedule::CronJobMeta {
                        name: #name_lit,
                        trigger: #trigger_tokens,
                        run: |__container| {
                            ::std::boxed::Box::pin(async move {
                                let __job = <Self>::from_container(__container);
                                ::nestrs_schedule::Scheduled::run(&__job).await
                            })
                        },
                    },
                )
            }
        }
    }
    .into()
}

/// The parsed trigger argument. `Interval`/`Timeout` carry whole milliseconds
/// computed at expansion time; `Cron` keeps the expression as a `syn::Expr` (a
/// string literal or a `CronExpression::X` path) plus an optional timezone
/// literal, emitted as `&'static str`s the `Scheduler` parses at boot.
enum Trigger {
    Interval(u64),
    Timeout(u64),
    Cron { expr: Expr, tz: Option<LitStr> },
}

impl Trigger {
    fn parse(args: TokenStream2) -> syn::Result<Self> {
        let metas = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(args)?;

        let mut every: Option<LitStr> = None;
        let mut after: Option<LitStr> = None;
        let mut cron: Option<Expr> = None;
        let mut tz: Option<LitStr> = None;

        for meta in metas {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            match key.as_str() {
                "every" => every = Some(as_str_lit(&meta.value, "every")?),
                "after" => after = Some(as_str_lit(&meta.value, "after")?),
                "tz" => tz = Some(as_str_lit(&meta.value, "tz")?),
                "cron" => cron = Some(meta.value),
                other => {
                    return Err(syn::Error::new_spanned(
                        &meta.path,
                        format!(
                            "unknown #[cron_job] argument `{other}`; \
                             expected `every`, `cron`, `after`, or `tz`"
                        ),
                    ))
                }
            }
        }

        let chosen = every.is_some() as u8 + after.is_some() as u8 + cron.is_some() as u8;
        if chosen == 0 {
            return Err(err(
                "#[cron_job] needs one trigger: `every = \"30s\"` (interval), \
                 `cron = \"...\"` (cron expression), or `after = \"10s\"` (one-shot)",
            ));
        }
        if chosen > 1 {
            return Err(err(
                "#[cron_job] takes exactly one of `every`, `cron`, or `after` — they are \
                 mutually exclusive",
            ));
        }
        if tz.is_some() && cron.is_none() {
            return Err(err("#[cron_job] `tz` is only valid alongside `cron`"));
        }

        if let Some(lit) = every {
            return Ok(Trigger::Interval(period_millis(&lit)?));
        }
        if let Some(lit) = after {
            return Ok(Trigger::Timeout(period_millis(&lit)?));
        }

        let expr = cron.expect("cron is set when every/after are not");
        // A string literal can be validated now; a `CronExpression::X` path is a
        // const resolved at the call site, so its validation waits for boot.
        if let Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) = &expr
        {
            if let Err(e) = croner::Cron::from_str(&s.value()) {
                return Err(syn::Error::new(
                    s.span(),
                    format!("invalid cron expression: {e}"),
                ));
            }
        }
        Ok(Trigger::Cron { expr, tz })
    }

    fn to_tokens(&self) -> TokenStream2 {
        match self {
            Trigger::Interval(ms) => quote! {
                ::nestrs_schedule::Trigger::Interval(::std::time::Duration::from_millis(#ms))
            },
            Trigger::Timeout(ms) => quote! {
                ::nestrs_schedule::Trigger::Timeout(::std::time::Duration::from_millis(#ms))
            },
            Trigger::Cron { expr, tz } => {
                let tz_tokens = match tz {
                    Some(lit) => quote! { ::std::option::Option::Some(#lit) },
                    None => quote! { ::std::option::Option::None },
                };
                quote! {
                    ::nestrs_schedule::Trigger::Cron { expr: #expr, tz: #tz_tokens }
                }
            }
        }
    }
}

/// Extract a string literal from an argument value, naming the key on mismatch.
fn as_str_lit(value: &Expr, key: &str) -> syn::Result<LitStr> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = value
    {
        Ok(s.clone())
    } else {
        Err(syn::Error::new_spanned(
            value,
            format!("#[cron_job] `{key}` must be a string literal, e.g. `{key} = \"...\"`"),
        ))
    }
}

fn err(message: &str) -> syn::Error {
    syn::Error::new(proc_macro2::Span::call_site(), message)
}

/// The duration in whole milliseconds for an `every`/`after` literal, parsed at
/// macro-expansion time so a bad literal is a compile error. Accepts an
/// `ms`/`s`/`m`/`h` suffix.
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
    // `ms` before the single-char `s` so "500ms" is not mis-read as "500m".
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
