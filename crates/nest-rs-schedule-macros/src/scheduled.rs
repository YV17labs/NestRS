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

use nest_rs_codegen::{
    DecoratorPair, Edge, TRANSACTIONAL, duplicate_argument, impl_self_ident,
    job_argument_needs_a_value, job_transaction, require_str_lit, transactional_value,
    unknown_argument,
};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, ExprLit, ImplItem, Lit, LitStr, Meta, MetaNameValue, Token};

/// The scheduled-tasks host keeps its own `#[injectable]`; this names the shape
/// `#[scheduled]` wants rather than reporting syn's `expected impl`.
const SCHEDULED_PAIR: DecoratorPair =
    DecoratorPair::on_provider("#[scheduled]", "#[every] / #[cron] / #[after]");

pub(crate) fn scheduled(args: TokenStream, input: TokenStream) -> TokenStream {
    if let Err(err) = reject_args(args) {
        return err.to_compile_error().into();
    }

    let mut item = match SCHEDULED_PAIR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let provider_name = match impl_self_ident(&self_ty, "#[scheduled]") {
        Ok(ident) => ident.to_string(),
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

        let (trigger_tokens, transactional) = match parse_trigger(&trigger_attr) {
            Ok(parsed) => parsed,
            Err(err) => return err.to_compile_error().into(),
        };
        let transaction_tokens = job_transaction(transactional, &quote!(::nest_rs_schedule));

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.to_string();

        submissions.push(quote! {
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_schedule::ScheduledMethod {
                    provider: #provider_name,
                    method: #method_name,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    trigger: #trigger_tokens,
                    transaction: #transaction_tokens,
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

/// `#[scheduled]` takes no arguments — the triggers are on the methods it
/// collects. It used to *ignore* whatever it was handed; `version` is called out
/// first because it is the one key a developer arrives with from
/// `#[controller(version = "1")]`, and this transport has an answer of its own
/// rather than a spelling correction.
fn reject_args(args: TokenStream) -> syn::Result<()> {
    let args = TokenStream2::from(args);
    Edge::Schedule.reject_version(&args)?;
    if args.is_empty() {
        return Ok(());
    }
    Err(syn::Error::new_spanned(
        &args,
        "#[scheduled] takes no arguments; tag methods with `#[every(\"...\")]`, \
         `#[cron(...)]` or `#[after(\"...\")]`",
    ))
}

fn is_trigger_attr(path: &syn::Path) -> bool {
    path.is_ident("cron") || path.is_ident("every") || path.is_ident("after")
}

/// The trigger tokens, plus whatever the shared `transactional` key said.
///
/// All three triggers take the key in the same place — after the trigger's own
/// argument, as a named value — so `#[every("30s", transactional = false)]`,
/// `#[after(..)]` and `#[cron(.., tz = .., transactional = false)]` are one
/// grammar rather than three that happen to spell a word alike.
fn parse_trigger(attr: &Attribute) -> syn::Result<(TokenStream2, Option<bool>)> {
    let key = attr
        .path()
        .get_ident()
        .map(ToString::to_string)
        .unwrap_or_default();
    match key.as_str() {
        "every" => {
            let (lit, transactional) = parse_period(attr, &key)?;
            let ms = period_millis(&lit)?;
            Ok((
                quote! {
                    ::nest_rs_schedule::Trigger::Interval(
                        ::std::time::Duration::from_millis(#ms)
                    )
                },
                transactional,
            ))
        }
        "after" => {
            let (lit, transactional) = parse_period(attr, &key)?;
            let ms = period_millis(&lit)?;
            Ok((
                quote! {
                    ::nest_rs_schedule::Trigger::Timeout(
                        ::std::time::Duration::from_millis(#ms)
                    )
                },
                transactional,
            ))
        }
        "cron" => parse_cron(attr),
        _ => unreachable!("is_trigger_attr filtered the attribute set"),
    }
}

/// The tokens inside `#[<key>(…)]`, or `expects` as the error when the
/// attribute carries no list at all.
fn list_tokens(attr: &Attribute, expects: String) -> syn::Result<TokenStream2> {
    Ok(attr
        .meta
        .require_list()
        .map_err(|_| syn::Error::new(attr.span(), expects))?
        .tokens
        .clone())
}

/// `#[every("30s")]` / `#[after("10s")]`, with the optional shared key after it.
fn parse_period(attr: &Attribute, key: &str) -> syn::Result<(LitStr, Option<bool>)> {
    let tokens = list_tokens(
        attr,
        format!(
            "#[{key}] expects `#[{key}(\"30s\")]`, optionally followed by `{TRANSACTIONAL} = false`"
        ),
    )?;
    let parser = |stream: syn::parse::ParseStream<'_>| -> syn::Result<(LitStr, Option<bool>)> {
        let lit: LitStr = stream.parse()?;
        let (transactional, _) = parse_trailing_keys(stream, key, None)?;
        Ok((lit, transactional))
    };
    parser.parse2(tokens)
}

/// The named keys a trigger accepts after its own argument: the shared
/// `transactional`, plus the one key the trigger itself may own (`tz`, on
/// `#[cron]`). Returns both, so the one "unknown key" sentence is worded here
/// and lists exactly what this trigger takes.
///
/// **A repeated key is refused, not last-write-wins.** `#[cron("…", tz = "A",
/// tz = "B")]` has no reading a developer could have meant, and accepting it
/// silently drops one of two declarations — which is the shape of defect this
/// whole grammar was unified to remove.
fn parse_trailing_keys(
    stream: syn::parse::ParseStream<'_>,
    key: &str,
    extra: Option<&str>,
) -> syn::Result<(Option<bool>, Option<MetaNameValue>)> {
    let mut transactional: Option<bool> = None;
    let mut owned: Option<MetaNameValue> = None;
    if !stream.peek(Token![,]) {
        return Ok((transactional, owned));
    }
    stream.parse::<Token![,]>()?;
    // Allow a trailing comma.
    if stream.is_empty() {
        return Ok((transactional, owned));
    }
    // `Meta`, not `MetaNameValue`: a bare `transactional` is a legal `Meta::Path`
    // and reaches the loop, where it earns a sentence naming the key. Parsed as
    // `MetaNameValue` it failed the whole `Punctuated`, and syn reported
    // `expected `=`` against the enclosing `#[scheduled]` — the *other* half of
    // the pair, and a decorator the developer had not touched.
    let metas: Punctuated<Meta, Token![,]> = Punctuated::parse_terminated(stream)?;
    for meta in metas {
        let path = meta.path().clone();
        // `get_ident` is `None` for a multi-segment path (`a::b`), and defaulting
        // made the refusal name the empty string — a sentence that refuses
        // without saying what it refused.
        let name = path
            .get_ident()
            .map(ToString::to_string)
            .unwrap_or_else(|| quote!(#path).to_string().replace(' ', ""));
        let known = name == TRANSACTIONAL || extra == Some(name.as_str());
        if !known {
            let mut accepted = Vec::new();
            if let Some(own) = extra {
                accepted.push(own);
            }
            accepted.push(TRANSACTIONAL);
            return Err(syn::Error::new_spanned(
                &path,
                unknown_argument(key, &name, &accepted),
            ));
        }
        let Meta::NameValue(meta) = meta else {
            return Err(syn::Error::new_spanned(
                &path,
                job_argument_needs_a_value(key, &name),
            ));
        };
        let taken = if name == TRANSACTIONAL {
            transactional.is_some()
        } else {
            owned.is_some()
        };
        if taken {
            return Err(syn::Error::new_spanned(
                &meta.path,
                duplicate_argument(key, &name),
            ));
        }
        if name == TRANSACTIONAL {
            transactional = Some(transactional_value(&meta.value)?);
        } else {
            owned = Some(meta);
        }
    }
    Ok((transactional, owned))
}

fn parse_cron(attr: &Attribute) -> syn::Result<(TokenStream2, Option<bool>)> {
    let tokens = list_tokens(
        attr,
        format!(
            "#[cron] expects `#[cron(\"...\")]` or \
             `#[cron(CronExpression::EVERY_MINUTE)]`, optionally followed by \
             `tz = \"Europe/Paris\"` and `{TRANSACTIONAL} = false`"
        ),
    )?;

    let parser =
        |stream: syn::parse::ParseStream<'_>| -> syn::Result<(Expr, Option<LitStr>, Option<bool>)> {
            let expr: Expr = stream.parse()?;
            let (transactional, tz) = parse_trailing_keys(stream, "cron", Some("tz"))?;
            let tz = tz
                .map(|meta| require_str_lit(&meta.value, "cron", "tz", "..."))
                .transpose()?;
            Ok((expr, tz, transactional))
        };
    let (expr, tz, transactional) = parser.parse2(tokens)?;

    // Literal cron expressions validate now; `CronExpression::X` paths wait
    // for boot (the `Scheduler::configure` call).
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = &expr
    {
        validate_cron_literal(s)?;
    }
    let tz_tokens = match tz {
        Some(lit) => quote! { ::std::option::Option::Some(#lit) },
        None => quote! { ::std::option::Option::None },
    };
    Ok((
        quote! {
            ::nest_rs_schedule::Trigger::Cron { expr: #expr, tz: #tz_tokens }
        },
        transactional,
    ))
}

/// Validate a literal cron expression at macro-expansion time, so a bad
/// expression is a compile error (spanned at the literal) rather than a
/// boot-time surprise. `CronExpression::X` paths are not literals and validate
/// at boot instead.
fn validate_cron_literal(s: &LitStr) -> syn::Result<()> {
    croner::Cron::from_str(&s.value())
        .map(|_| ())
        .map_err(|e| syn::Error::new(s.span(), format!("invalid cron expression: {e}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;

    fn lit(s: &str) -> LitStr {
        LitStr::new(s, Span::call_site())
    }

    #[test]
    fn valid_cron_literal_passes() {
        validate_cron_literal(&lit("0 0 * * *")).expect("a well-formed cron literal validates");
    }

    #[test]
    fn invalid_cron_literal_is_a_compile_error() {
        let err = validate_cron_literal(&lit("not a cron expression"))
            .expect_err("a malformed cron literal must fail at macro expansion");
        assert!(
            err.to_string().contains("invalid cron expression"),
            "error names the problem, got: {err}",
        );
    }

    #[test]
    fn period_millis_parses_each_suffix() {
        assert_eq!(period_millis(&lit("500ms")).unwrap(), 500);
        assert_eq!(period_millis(&lit("30s")).unwrap(), 30_000);
        assert_eq!(period_millis(&lit("5m")).unwrap(), 300_000);
        assert_eq!(period_millis(&lit("1h")).unwrap(), 3_600_000);
    }

    #[test]
    fn period_millis_rejects_zero_and_missing_suffix() {
        assert!(period_millis(&lit("0s")).is_err());
        assert!(period_millis(&lit("10")).is_err());
    }
}
