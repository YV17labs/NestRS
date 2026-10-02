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
    DecoratorPair, Edge, HostBorrow, JobDecorator, JobKey, await_if_async, cfg_attrs,
    duration_millis, impl_self_ident, job_argument_needs_a_value, job_key, job_keys,
    job_returns_a_result, job_transaction, replicas_default, replicas_value, require_str_lit,
    returns_unit, shared_receiver, site, takes_value, transactional_value, ungrouped_expr,
    unread_job_key,
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
        // the one the shape refusals name — as `#[on_event]`'s refusals name it.
        let key = nest_rs_codegen::key_as_written(trigger_attr.path());
        let member = JobDecorator::named(&key)
            .unwrap_or_else(|| unreachable!("one_role_per_method matched a trigger attribute"));
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
            return syn::Error::new_spanned(&method.sig, job_returns_a_result(member))
                .to_compile_error()
                .into();
        }

        // Every method's keys are read before the first refusal is returned, so
        // a host with several wrong triggers learns about all of them at once.
        let ParsedTrigger {
            trigger: trigger_tokens,
            transactional,
            replicas: replicas_tokens,
        } = match parse_trigger(&trigger_attr, member) {
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
                    replicas: #replicas_tokens,
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
    /// The `Replicas` variant, every replica when the key is unwritten.
    replicas: TokenStream2,
}

/// The keys a trigger wrote after its own argument.
struct TrailingKeys {
    transactional: Option<bool>,
    /// The `Replicas` variant the key selected, `None` when unwritten.
    replicas: Option<TokenStream2>,
    /// The `tz` a calendar is read in — `#[cron]`'s alone, which the table
    /// guarantees by refusing it at the other two.
    tz: Option<MetaNameValue>,
}

/// The trigger tokens, plus whatever the shared keys said.
///
/// All three triggers take their keys in the same place — after the trigger's
/// own argument, as named values — so `#[every("30s", transactional = false)]`,
/// `#[after(..)]` and `#[cron(.., tz = .., replicas = "one")]` are one grammar
/// rather than three that happen to spell a word alike.
fn parse_trigger(attr: &Attribute, member: JobDecorator) -> syn::Result<ParsedTrigger> {
    let resolved = |replicas: Option<TokenStream2>| {
        replicas.unwrap_or_else(|| replicas_default(&quote!(::nest_rs_schedule)))
    };
    let (trigger, keys) = match member {
        JobDecorator::Every | JobDecorator::After => {
            let (period, keys) = parse_period(attr, member)?;
            let ms = duration_millis(member.name(), None, &period)?;
            let trigger = if member == JobDecorator::Every {
                quote! {
                    ::nest_rs_schedule::Trigger::Interval(::std::time::Duration::from_millis(#ms))
                }
            } else {
                quote! {
                    ::nest_rs_schedule::Trigger::Timeout(::std::time::Duration::from_millis(#ms))
                }
            };
            (trigger, keys)
        }
        JobDecorator::Cron => parse_cron(attr)?,
        JobDecorator::Process => unreachable!("#[scheduled] collects triggers, never #[process]"),
    };
    Ok(ParsedTrigger {
        trigger,
        transactional: keys.transactional,
        replicas: resolved(keys.replicas),
    })
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
fn parse_period(attr: &Attribute, member: JobDecorator) -> syn::Result<(Expr, TrailingKeys)> {
    let key = member.name();
    let tokens = list_tokens(
        attr,
        format!(
            "#[{key}] expects `#[{key}(\"30s\")]`, optionally followed by {}",
            optional_keys(member)
        ),
    )?;
    let parser = |stream: syn::parse::ParseStream<'_>| -> syn::Result<(Expr, TrailingKeys)> {
        let period: Expr = stream.parse()?;
        let keys = parse_trailing_keys(stream, member)?;
        Ok((period, keys))
    };
    parser.parse2(tokens)
}

/// The keys `member` takes after its own argument, each written as the
/// family's table gives it — so the sentence offers exactly what the parser
/// reads, and a key the table gives a trigger reaches the sentence with it.
fn optional_keys(member: JobDecorator) -> String {
    let written: Vec<String> = job_keys(member)
        .map(|key| format!("`{}`", key.example()))
        .collect();
    match written.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// The named keys a trigger accepts after its own argument, read against the
/// family's table (`nest_rs_codegen::job_key`): a key this trigger takes, a key
/// another member takes and this trigger cannot — `replicas` on the one-shot
/// `#[after]`, `retries` on any — refused naming why, or a key no member takes,
/// refused listing this trigger's column.
///
/// **A repeated key is refused, not last-write-wins.** `#[cron("…", tz = "A",
/// tz = "B")]` has no reading a developer could have meant, and accepting it
/// silently drops one of two declarations — which is the shape of defect this
/// whole grammar was unified to remove.
fn parse_trailing_keys(
    stream: syn::parse::ParseStream<'_>,
    member: JobDecorator,
) -> syn::Result<TrailingKeys> {
    let mut keys = TrailingKeys {
        transactional: None,
        replicas: None,
        tz: None,
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
    let mut written = nest_rs_codegen::WrittenKeys::default();
    for meta in metas {
        let path = meta.path().clone();
        let name = nest_rs_codegen::key_as_written(&path);
        // Before the value is looked at: a key refused here — unknown, another
        // member's, or written twice — is refused whatever it was given.
        let key = job_key(member, &mut written, &name, &path)?;
        let Meta::NameValue(meta) = meta else {
            return Err(syn::Error::new_spanned(
                &path,
                job_argument_needs_a_value(member, &name),
            ));
        };
        match key {
            JobKey::Transactional => {
                keys.transactional = Some(transactional_value(member, &meta.value)?);
            }
            JobKey::Replicas => {
                keys.replicas = Some(replicas_value(
                    member,
                    &meta.value,
                    &quote!(::nest_rs_schedule),
                )?);
            }
            JobKey::Tz => {
                keys.tz = Some(meta);
            }
            unread @ (JobKey::Queue | JobKey::Retries | JobKey::Concurrency | JobKey::Throttle) => {
                return Err(unread_job_key(member, unread, &path));
            }
        }
    }
    Ok(keys)
}

fn parse_cron(attr: &Attribute) -> syn::Result<(TokenStream2, TrailingKeys)> {
    let tokens = list_tokens(
        attr,
        format!(
            "#[cron] expects `#[cron(\"...\")]` or \
             `#[cron(CronExpression::EVERY_MINUTE)]`, optionally followed by {}",
            optional_keys(JobDecorator::Cron)
        ),
    )?;

    let parser =
        |stream: syn::parse::ParseStream<'_>| -> syn::Result<(Expr, Option<LitStr>, TrailingKeys)> {
            let expr: Expr = stream.parse()?;
            let mut keys = parse_trailing_keys(stream, JobDecorator::Cron)?;
            let tz = keys
                .tz
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
    // for boot (the `Scheduler::configure` call). A literal of any other kind is
    // no expression at all, and is refused here rather than as a type mismatch
    // inside the expansion. Read through the invisible group a `macro_rules!`
    // forwards a value in, as every value reader is.
    match ungrouped_expr(&expr) {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => validate_cron_literal(s)?,
        Expr::Lit(other) => {
            return Err(syn::Error::new_spanned(
                other,
                takes_value(
                    "cron",
                    None,
                    "a cron expression — a string literal or a `CronExpression` constant, e.g. \
                     `#[cron(\"0 0 * * *\")]` or `#[cron(CronExpression::EVERY_HOUR)]`",
                ),
            ));
        }
        _ => {}
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
            "{}: {name:?} is not an IANA time zone name — it takes an `Area/Location` \
             identifier from the IANA time zone database (e.g. \"Europe/Paris\", \
             \"America/New_York\", \"UTC\")",
            site("cron", Some("tz")),
        ),
    ))
}

/// Validate a literal cron expression at macro-expansion time, so a bad
/// expression is a compile error (spanned at the literal) rather than a
/// boot-time surprise. `CronExpression::X` paths are not literals and validate
/// at boot instead.
fn validate_cron_literal(s: &LitStr) -> syn::Result<()> {
    croner::Cron::from_str(&s.value()).map(|_| ()).map_err(|e| {
        syn::Error::new(
            s.span(),
            format!(
                "{}: {:?} is not a cron expression: {e}",
                site("cron", None),
                s.value()
            ),
        )
    })
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

    /// Every key the job-key table gives a trigger is read by this parser,
    /// written as the table's own example — the spelling the "optionally
    /// followed by" sentence offers a developer to paste.
    #[test]
    fn every_key_of_each_triggers_column_is_read() {
        for name in TRIGGER_ATTRS {
            let member =
                JobDecorator::named(name).expect("a trigger is a member of the job family");
            let own = match member {
                JobDecorator::Cron => quote!("0 0 * * *"),
                _ => quote!("30s"),
            };
            let attr_name = syn::Ident::new(name, Span::call_site());
            for key in job_keys(member) {
                let written: TokenStream2 = key.example().parse().expect("an example is tokens");
                let attr: Attribute = syn::parse_quote!(#[#attr_name(#own, #written)]);
                if let Err(refusal) = parse_trigger(&attr, member) {
                    panic!("#[{name}] does not read `{}`: {refusal}", key.name());
                }
            }
        }
    }

    #[test]
    fn invalid_cron_literal_is_a_compile_error() {
        let err = validate_cron_literal(&lit("not a cron expression"))
            .expect_err("a malformed cron literal must fail at macro expansion");
        assert!(
            err.to_string()
                .starts_with("#[cron]: \"not a cron expression\" is not a cron expression: "),
            "error opens with its site and names the problem, got: {err}",
        );
    }
}
