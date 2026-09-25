//! `#[scheduled]` — orchestrator on a provider's `impl` block. Walks the
//! methods, finds those tagged with `#[cron(...)]` / `#[every("...")]` /
//! `#[after("...")]`, strips the attribute, and submits one
//! `ScheduledMethod` per method to the link-time inventory. The methods stay
//! on the impl block unchanged so they remain regular methods callable
//! from anywhere.
//!
//! Discoverable is NOT emitted here — the provider's own `#[injectable]` owns
//! it. Inventory is exactly the seam `#[hooks]` uses for lifecycle methods,
//! for the same reason.

use std::str::FromStr;

use nest_rs_codegen::{
    DecoratorPair, Edge, HostBorrow, TRANSACTIONAL, await_if_async, cfg_attrs, duplicate_argument,
    duration_millis, impl_self_ident, job_argument_needs_a_value, job_argument_refused,
    job_returns_a_result, job_transaction, require_str_lit, returns_unit, shared_receiver,
    transactional_value, unknown_argument,
};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ext::IdentExt;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, ExprLit, ImplItem, Lit, LitStr, Meta, MetaNameValue, Token};

/// The scheduled-tasks host keeps its own `#[injectable]`; this names the shape
/// `#[scheduled]` wants rather than reporting syn's `expected impl`.
const SCHEDULED_PAIR: DecoratorPair =
    DecoratorPair::on_provider("#[scheduled]", "#[every] / #[cron] / #[after]");

pub(crate) fn scheduled(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    SCHEDULED_PAIR
        .keep_item_on_refusal(written, expansion, &TRIGGER_ATTRS, |_| TokenStream2::new())
        .into()
}

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    if let Err(err) = reject_args(args) {
        return err.to_compile_error().into();
    }

    let mut item = match SCHEDULED_PAIR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let provider = match impl_self_ident(&self_ty, "#[scheduled]") {
        Ok(ident) => ident,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_name = provider.unraw().to_string();

    let mut submissions: Vec<TokenStream2> = Vec::new();
    let mut refusals: Option<syn::Error> = None;

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let index = match nest_rs_codegen::one_role_per_method(
            "trigger",
            &method.attrs,
            &TRIGGER_ATTRS,
            "",
        ) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(err) => return err.to_compile_error().into(),
        };
        let trigger_attr = method.attrs.remove(index);

        // The trigger as written is the attribute the reader looks at, so it is
        // the one the shape refusals name — as `#[on_event]`'s name `#[on_event]`.
        let key = nest_rs_codegen::key_as_written(trigger_attr.path());
        let written = format!("#[{key}]");
        if let Err(err) = shared_receiver(method, &written, &provider, HostBorrow::Arc) {
            return err.to_compile_error().into();
        }
        if let Err(err) = nest_rs_codegen::concrete_signature(method, &written) {
            return err.to_compile_error().into();
        }
        // The expansion hands the method's answer to the scheduler, which reads
        // `Ok` and `Err`; a `()` would surface as a type mismatch inside it.
        if returns_unit(&method.sig.output) {
            return syn::Error::new_spanned(&method.sig, job_returns_a_result(&key))
                .to_compile_error()
                .into();
        }

        // Every method's keys are read before the first refusal is returned, so
        // a host with several wrong triggers learns about all of them at once.
        let ParsedTrigger {
            trigger: trigger_tokens,
            transactional,
        } = match parse_trigger(&trigger_attr) {
            Ok(parsed) => parsed,
            Err(err) => {
                match &mut refusals {
                    Some(refusals) => refusals.combine(err),
                    None => refusals = Some(err),
                }
                continue;
            }
        };
        let transaction_tokens = job_transaction(transactional, &quote!(::nest_rs_schedule));

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.unraw().to_string();
        let cfgs = cfg_attrs(&method.attrs);
        let call = await_if_async(&method.sig, quote!(<#self_ty>::#method_ident(&__provider)));

        submissions.push(quote! {
            #(#cfgs)*
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_schedule::ScheduledMethod {
                    origin: ::core::module_path!(),
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
                        #call
                    }),
                }
            }
        });
    }

    if let Some(refusals) = refusals {
        return refusals.to_compile_error().into();
    }
    let host_check = SCHEDULED_PAIR.provider_host_check(&self_ty);
    let out = quote! {
        #item
        #host_check
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
    SCHEDULED_PAIR.reject_args(&args, "the provider's scope is declared by")
}

/// The closed trigger vocabulary, read by the one-role helper both to find a
/// method's trigger and to list what is accepted — so the set a method is
/// checked against and the set it is told about cannot disagree.
const TRIGGER_ATTRS: [&str; 3] = ["cron", "every", "after"];

/// What a trigger attribute declared: the trigger itself, and the shared keys a
/// trigger may carry after its own argument.
struct ParsedTrigger {
    trigger: TokenStream2,
    /// The shared `transactional` key, `None` when unwritten.
    transactional: Option<bool>,
}

/// The keys a trigger wrote after its own argument.
struct TrailingKeys {
    transactional: Option<bool>,
    /// The one key the trigger itself owns — `tz`, on `#[cron]`.
    owned: Option<MetaNameValue>,
}

/// The trigger tokens, plus whatever the shared keys said.
///
/// All three triggers take their keys in the same place — after the trigger's
/// own argument, as named values — so `#[every("30s", transactional = false)]`,
/// `#[after(..)]` and `#[cron(.., tz = .., transactional = false)]` are one
/// grammar rather than three that happen to spell a word alike.
fn parse_trigger(attr: &Attribute) -> syn::Result<ParsedTrigger> {
    let key = attr
        .path()
        .get_ident()
        .map(ToString::to_string)
        .unwrap_or_default();
    match key.as_str() {
        "every" => {
            let (period, keys) = parse_period(attr, &key)?;
            let ms = duration_millis(&key, None, &period)?;
            Ok(ParsedTrigger {
                trigger: quote! {
                    ::nest_rs_schedule::Trigger::Interval(
                        ::std::time::Duration::from_millis(#ms)
                    )
                },
                transactional: keys.transactional,
            })
        }
        "after" => {
            let (period, keys) = parse_period(attr, &key)?;
            let ms = duration_millis(&key, None, &period)?;
            Ok(ParsedTrigger {
                trigger: quote! {
                    ::nest_rs_schedule::Trigger::Timeout(
                        ::std::time::Duration::from_millis(#ms)
                    )
                },
                transactional: keys.transactional,
            })
        }
        "cron" => {
            let (trigger, keys) = parse_cron(attr)?;
            Ok(ParsedTrigger {
                trigger,
                transactional: keys.transactional,
            })
        }
        _ => unreachable!("one_role_per_method matched a trigger attribute"),
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

/// `#[every("30s")]` / `#[after("10s")]`, with the optional shared keys after it.
///
/// The period is read as any expression and judged by the duration grammar, so
/// `#[every(30)]` gets the grammar's sentence rather than syn's bare "expected
/// string literal".
fn parse_period(attr: &Attribute, key: &str) -> syn::Result<(Expr, TrailingKeys)> {
    let tokens = list_tokens(
        attr,
        format!(
            "#[{key}] expects `#[{key}(\"30s\")]`, optionally followed by `{TRANSACTIONAL} = false`"
        ),
    )?;
    let parser = |stream: syn::parse::ParseStream<'_>| -> syn::Result<(Expr, TrailingKeys)> {
        let period: Expr = stream.parse()?;
        let keys = parse_trailing_keys(stream, key, None)?;
        Ok((period, keys))
    };
    parser.parse2(tokens)
}

/// The named keys a trigger accepts after its own argument: the shared
/// `transactional` plus the one key the trigger itself may own (`tz`, on
/// `#[cron]`). A key another member of the job family takes and this trigger
/// cannot — `retries` on any — is refused through `job_argument_refused`, naming
/// why. Returned together, so the one "unknown key" sentence is worded here and
/// lists exactly what this trigger takes.
///
/// **A repeated key is refused, not last-write-wins.** `#[cron("…", tz = "A",
/// tz = "B")]` has no reading a developer could have meant, and accepting it
/// silently drops one of two declarations — which is the shape of defect this
/// whole grammar was unified to remove.
fn parse_trailing_keys(
    stream: syn::parse::ParseStream<'_>,
    key: &str,
    extra: Option<&str>,
) -> syn::Result<TrailingKeys> {
    let mut keys = TrailingKeys {
        transactional: None,
        owned: None,
    };
    if !stream.peek(Token![,]) {
        return Ok(keys);
    }
    stream.parse::<Token![,]>()?;
    // Allow a trailing comma.
    if stream.is_empty() {
        return Ok(keys);
    }
    // `Meta`, not `MetaNameValue`: a bare `transactional` is a legal `Meta::Path`
    // and reaches the loop, where it earns a sentence naming the key. Parsed as
    // `MetaNameValue` it failed the whole `Punctuated`, and syn reported
    // `expected `=`` against the enclosing `#[scheduled]` — the *other* half of
    // the pair, and a decorator the developer had not touched.
    let metas: Punctuated<Meta, Token![,]> = Punctuated::parse_terminated(stream)?;
    for meta in metas {
        let path = meta.path().clone();
        let name = nest_rs_codegen::key_as_written(&path);
        // Before the unknown-key check, and whatever the value: a key another
        // member of the job family takes is not misspelled here, it is
        // meaningless, and the family's sentence says why.
        if let Some(refusal) = job_argument_refused(key, &name) {
            return Err(syn::Error::new_spanned(&path, refusal));
        }
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
            keys.transactional.is_some()
        } else {
            keys.owned.is_some()
        };
        if taken {
            return Err(syn::Error::new_spanned(
                &meta.path,
                duplicate_argument(key, &name),
            ));
        }
        if name == TRANSACTIONAL {
            keys.transactional = Some(transactional_value(&meta.value)?);
        } else {
            keys.owned = Some(meta);
        }
    }
    Ok(keys)
}

fn parse_cron(attr: &Attribute) -> syn::Result<(TokenStream2, TrailingKeys)> {
    let tokens = list_tokens(
        attr,
        format!(
            "#[cron] expects `#[cron(\"...\")]` or \
             `#[cron(CronExpression::EVERY_MINUTE)]`, optionally followed by \
             `tz = \"Europe/Paris\"` and `{TRANSACTIONAL} = false`"
        ),
    )?;

    let parser =
        |stream: syn::parse::ParseStream<'_>| -> syn::Result<(Expr, Option<LitStr>, TrailingKeys)> {
            let expr: Expr = stream.parse()?;
            let mut keys = parse_trailing_keys(stream, "cron", Some("tz"))?;
            let tz = keys
                .owned
                .take()
                .map(|meta| require_str_lit(&meta.value, "cron", "tz", "Europe/Paris"))
                .transpose()?;
            if let Some(name) = &tz {
                validate_timezone_literal(name)?;
            }
            Ok((expr, tz, keys))
        };
    let (expr, tz, keys) = parser.parse2(tokens)?;

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
        keys,
    ))
}

/// The IANA name set is closed and `tz` is always a literal, so a typo is
/// knowable here — the same fact `validate_cron_literal` acts on for the key
/// beside it. Boot-time resolution stays in `Scheduler::configure`: it is what
/// the *value* is finally parsed by, and a second reader is not a second
/// authority when the first only ever refuses.
fn validate_timezone_literal(s: &LitStr) -> syn::Result<()> {
    let name = s.value();
    if name.parse::<chrono_tz::Tz>().is_ok() {
        return Ok(());
    }
    // Name the fact a reader can check, and point at the register rather than
    // listing 600 names into a compiler diagnostic.
    Err(syn::Error::new(
        s.span(),
        format!(
            "`{name}` is not an IANA time zone name — `tz` takes a `Area/Location` \
             identifier from the IANA time zone database (e.g. \"Europe/Paris\", \
             \"America/New_York\", \"UTC\")",
        ),
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
}
