//! `#[scheduled]` — orchestrator on a provider's `impl` block: strips each
//! method's trigger attribute and submits one `ScheduledMethod` per method to the
//! link-time inventory, leaving the methods unchanged. The provider's own
//! `#[injectable]` owns `Discoverable`.

use nest_rs_codegen::pair;
use std::str::FromStr;

use nest_rs_codegen::{
    Edge, HostBorrow, JobDecorator, JobKey, Replicas, await_if_async, cfg_attrs, duration_millis,
    impl_self_ident, invalid_time_zone, job_key, job_keys, job_returns_a_result, job_timeout,
    job_transaction, key_value, key_without_replicas_one, replicas_value, require_str_lit,
    returns_unit, shared_receiver, site, takes_value, timeout_value, transactional_value,
    ungrouped_expr, unread_job_key,
};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::ext::IdentExt;
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, ExprLit, ImplItem, Lit, LitStr, Token};

pub(crate) fn scheduled(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::SCHEDULED
        .keep_item_on_refusal(written, expansion, &TRIGGER_ATTRS, |_| TokenStream2::new())
        .into()
}

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    if let Err(err) = reject_args(args) {
        return err.to_compile_error().into();
    }

    let mut item = match pair::SCHEDULED.parse_operations(input.into()) {
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

        // The shape refusals name the trigger as written.
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

        // Every method's keys are read before the first refusal is returned, so a
        // host with several wrong triggers learns about all of them at once.
        let ParsedTrigger {
            trigger: trigger_tokens,
            timeout,
            transactional,
            replicas,
            key,
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
        let timeout_tokens = job_timeout(timeout, &quote!(::nest_rs_schedule));
        let transaction_tokens = job_transaction(transactional, &quote!(::nest_rs_schedule));
        let replicas_tokens = replicas.tokens(&quote!(::nest_rs_schedule));
        let key_tokens = match key {
            Some(key) => quote! { ::std::option::Option::Some(#key) },
            None => quote! { ::std::option::Option::None },
        };

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
                    timeout: #timeout_tokens,
                    transaction: #transaction_tokens,
                    replicas: #replicas_tokens,
                    key: #key_tokens,
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
    let host_check = pair::SCHEDULED.provider_host_check(&self_ty);
    let out = quote! {
        #item
        #host_check
        #(#submissions)*
    };
    out.into()
}

/// `#[scheduled]` takes no arguments — the triggers are on the methods it
/// collects; `version` gets an answer of its own.
fn reject_args(args: TokenStream) -> syn::Result<()> {
    let args = TokenStream2::from(args);
    Edge::Schedule.reject_version(&args)?;
    pair::SCHEDULED.reject_args(&args, "the provider's scope is declared by")
}

/// The closed trigger vocabulary, read both to find a method's trigger and to
/// list what is accepted.
const TRIGGER_ATTRS: [&str; 3] = ["cron", "every", "after"];

/// What a trigger attribute declared: the trigger itself, and the shared keys a
/// trigger may carry after its own argument.
struct ParsedTrigger {
    trigger: TokenStream2,
    /// The shared `timeout` key in milliseconds, `None` when unwritten.
    timeout: Option<u64>,
    /// The shared `transactional` key, `None` when unwritten.
    transactional: Option<bool>,
    /// The replicas it fires on, every replica when the key is unwritten.
    replicas: Replicas,
    /// The identity it pins, `None` when it derives its own.
    key: Option<LitStr>,
}

/// The keys a trigger wrote after its own argument.
struct TrailingKeys {
    timeout: Option<u64>,
    transactional: Option<bool>,
    /// The replicas the key selected, `None` when unwritten.
    replicas: Option<Replicas>,
    /// The `tz` a calendar is read in — `#[cron]`'s alone.
    tz: Option<Expr>,
    /// The identity it pins, with the key as written for the refusal of one
    /// beside a job firing on every replica.
    key: Option<(syn::Path, LitStr)>,
}

/// The trigger tokens, plus whatever the shared keys said.
///
/// All three triggers take their keys after the trigger's own argument, as
/// named values: one grammar.
fn parse_trigger(attr: &Attribute, member: JobDecorator) -> syn::Result<ParsedTrigger> {
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
    let replicas = keys.replicas.unwrap_or_default();
    // Beside a job firing on every replica a key would be a declaration nothing
    // reads.
    let key = match keys.key {
        Some((path, _)) if replicas != Replicas::One => {
            return Err(syn::Error::new_spanned(
                path,
                key_without_replicas_one(member),
            ));
        }
        key => key.map(|(_, key)| key),
    };
    Ok(ParsedTrigger {
        trigger,
        timeout: keys.timeout,
        transactional: keys.transactional,
        replicas,
        key,
    })
}

/// The tokens inside `#[<key>(…)]`, or `expects` as the error when the
/// attribute carries no list at all.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
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
/// Read as any expression, so `#[every(30)]` gets the duration grammar's
/// sentence rather than syn's "expected string literal".
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

/// The keys `member` takes after its own argument, written as the family's
/// table gives them.
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
/// family's table (`nest_rs_codegen::job_key`): another member's key, or one no
/// member takes, is refused naming why.
///
/// **A repeated key is refused, not last-write-wins.**
fn parse_trailing_keys(
    stream: syn::parse::ParseStream<'_>,
    member: JobDecorator,
) -> syn::Result<TrailingKeys> {
    let mut keys = TrailingKeys {
        timeout: None,
        transactional: None,
        replicas: None,
        tz: None,
        key: None,
    };
    if !stream.peek(Token![,]) {
        return Ok(keys);
    }
    stream.parse::<Token![,]>()?;
    // Allow a trailing comma.
    if stream.is_empty() {
        return Ok(keys);
    }
    // Each key is judged before its value, so a bare one earns a sentence naming
    // the key rather than syn's `expected `=`` against the enclosing `#[scheduled]`.
    member.grammar().parse(stream, |arg| {
        match job_key(member, &arg)? {
            JobKey::Timeout => keys.timeout = Some(timeout_value(member, &arg.expr()?)?),
            JobKey::Transactional => {
                keys.transactional = Some(transactional_value(member, &arg.expr()?)?);
            }
            JobKey::Replicas => keys.replicas = Some(replicas_value(member, &arg.expr()?)?),
            JobKey::Key => {
                let at = syn::Path::from(arg.ident().clone());
                keys.key = Some((at, key_value(member, &arg.expr()?)?));
            }
            JobKey::Tz => keys.tz = Some(arg.expr()?),
            unread @ (JobKey::Queue | JobKey::Retries | JobKey::Concurrency | JobKey::Throttle) => {
                return Err(unread_job_key(member, unread, arg.ident()));
            }
        }
        Ok(())
    })?;
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
                .map(|value| require_str_lit(&value, "cron", "tz", "Europe/Paris"))
                .transpose()?;
            if let Some(name) = &tz {
                validate_timezone_literal(name)?;
            }
            Ok((expr, tz, keys))
        };
    let (expr, tz, keys) = parser.parse2(tokens)?;

    // Literal cron expressions validate now; `CronExpression::X` paths wait for
    // boot. Read through the invisible group a `macro_rules!` forwards a value in.
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
/// knowable here, in the database the scheduler reads.
fn validate_timezone_literal(s: &LitStr) -> syn::Result<()> {
    let name = s.value();
    if jiff_tzdb::get(&name).is_some() {
        return Ok(());
    }
    Err(syn::Error::new(s.span(), invalid_time_zone(&name)))
}

/// Validate a literal cron expression at expansion time, spanned at the
/// literal.
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

    #[test]
    fn a_key_is_read_beside_replicas_one_and_refused_beside_each() {
        let attr: Attribute =
            syn::parse_quote!(#[every("1h", replicas = "one", key = "billing::Tasks::close")]);
        let parsed = parse_trigger(&attr, JobDecorator::Every).expect("a pinned job firing once");
        assert_eq!(
            parsed.key.map(|key| key.value()).as_deref(),
            Some("billing::Tasks::close")
        );
        for attr in [
            syn::parse_quote!(#[every("1h", key = "billing::Tasks::close")]),
            syn::parse_quote!(#[every("1h", replicas = "each", key = "billing::Tasks::close")]),
            syn::parse_quote!(#[every("1h", key = "billing::Tasks::close", replicas = "each")]),
        ] {
            let attr: Attribute = attr;
            let refusal = parse_trigger(&attr, JobDecorator::Every)
                .err()
                .expect("a key beside a job firing on every replica")
                .to_string();
            assert_eq!(
                refusal,
                nest_rs_codegen::key_without_replicas_one(JobDecorator::Every)
            );
        }
    }

    fn lit(s: &str) -> LitStr {
        LitStr::new(s, Span::call_site())
    }

    #[test]
    fn valid_cron_literal_passes() {
        validate_cron_literal(&lit("0 0 * * *")).expect("a well-formed cron literal validates");
    }

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
                // `key` is read beside the declaration it needs, which the table
                // gives every member taking `key`.
                let example = match key {
                    JobKey::Key => format!("{}, {}", JobKey::Replicas.example(), key.example()),
                    _ => key.example().to_owned(),
                };
                let written: TokenStream2 = example.parse().expect("an example is tokens");
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
