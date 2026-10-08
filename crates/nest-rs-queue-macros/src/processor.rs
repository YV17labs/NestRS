//! `#[processor]` — orchestrator on a provider's `impl` block. Walks the
//! methods; for each one tagged `#[process(queue = …, …)]` emits a type-erased
//! handler and a `ProcessMethod` inventory submission the port's
//! `QueueWorker` runs from boot. No `Discoverable` for the host struct: the
//! user's own `#[injectable]` owns it.

use nest_rs_codegen::pair;
use nest_rs_codegen::{
    Edge, Grammar, JobDecorator, JobKey, PipeWrapper, await_if_async, cfg_attrs, duration_millis,
    generic_args, impl_self_ident, job_key, job_returns_a_result, job_timeout, job_transaction,
    missing_argument, payload_arg_type, pipe_wrapper, returns_unit, snake_case, takes_value,
    timeout_value, transactional_value, ungrouped_expr, unread_job_key,
};
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Expr, ExprLit, FnArg, Ident, ImplItem, Lit, LitStr, Type};

/// The member of the job family this decorator is — the column of
/// `nest_rs_codegen`'s job-key table its keys are read against.
const PROCESS: JobDecorator = JobDecorator::Process;

/// The two keys `throttle(..)` takes.
const THROTTLE: Grammar = Grammar::new("process", &["limit", "window"]).under("throttle");

/// Why a checkpoint cannot share the attempt's transaction — the refusal of a
/// `Checkpoint<_>` parameter on a transactional method.
const CHECKPOINT_NEEDS_THE_POOL: &str = "a `Checkpoint<_>` parameter needs `transactional = false` \
     on its #[process]: a checkpoint is saved to the queue backend at once, while a transactional \
     attempt rolls its database work back when it fails — the retry would resume past work that \
     was undone";

pub(crate) fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::PROCESSOR
        .keep_item_on_refusal(written, expansion, &["process"], |_| TokenStream2::new())
        .into()
}

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    if let Err(err) = reject_args(args) {
        return err.to_compile_error().into();
    }

    let mut item = match pair::PROCESSOR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let host_check = pair::PROCESSOR.provider_host_check(&self_ty);
    let provider_ident = match impl_self_ident(&self_ty, "#[processor]") {
        Ok(ident) => ident,
        Err(err) => return err.to_compile_error().into(),
    };

    let mut emissions: Vec<TokenStream2> = Vec::new();
    let mut refusals: Option<syn::Error> = None;

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let index =
            match nest_rs_codegen::one_role_per_method("queue", &method.attrs, &["process"], "") {
                Ok(Some(index)) => index,
                Ok(None) => continue,
                Err(err) => return err.to_compile_error().into(),
            };
        let attr = method.attrs.remove(index);
        if let Err(err) = nest_rs_codegen::concrete_signature(method, "#[process]") {
            return err.to_compile_error().into();
        }

        // A bare `#[process]` lacks the one key a method cannot do without: say
        // that, not syn's `expected attribute arguments in parentheses`.
        match &attr.meta {
            syn::Meta::List(_) => {}
            syn::Meta::Path(_) => {
                return syn::Error::new_spanned(&attr, missing_queue())
                    .to_compile_error()
                    .into();
            }
            syn::Meta::NameValue(_) => {
                return syn::Error::new_spanned(
                    &attr,
                    format!(
                        "#[process] takes its keys in a list — write `#[process({})]`",
                        JobKey::Queue.example(),
                    ),
                )
                .to_compile_error()
                .into();
            }
        }

        // Every method's keys are read before the first refusal is returned, so
        // a host with several wrong declarations learns about all of them at once.
        let args = match attr.parse_args::<ProcessArgs>() {
            Ok(a) => a,
            Err(err) => {
                match &mut refusals {
                    Some(refusals) => refusals.combine(err),
                    None => refusals = Some(err),
                }
                continue;
            }
        };

        if let Err(err) = check_shape(method) {
            return err.to_compile_error().into();
        }

        match emit_method(&self_ty, &provider_ident, method, args) {
            Ok(emitted) => emissions.push(emitted),
            Err(err) => return err.to_compile_error().into(),
        }
    }

    if let Some(refusals) = refusals {
        return refusals.to_compile_error().into();
    }
    let out = quote! {
        #item

        #host_check
        #(#emissions)*
    };
    out.into()
}

/// The shape every `#[process]` method has, refused with the rule rather than
/// with what rustc says of the code the expansion writes around it: its answer is
/// a `Result` the attempt reads — `Ok(())` completes the job, `Err` fails the
/// attempt. The receiver is read with the job argument, by `payload_arg_type`.
fn check_shape(method: &syn::ImplItemFn) -> syn::Result<()> {
    if returns_unit(&method.sig.output) {
        return Err(syn::Error::new_spanned(
            &method.sig,
            job_returns_a_result(PROCESS),
        ));
    }
    Ok(())
}

/// The handler and the inventory entry for one `#[process]` method.
fn emit_method(
    self_ty: &Type,
    provider: &Ident,
    method: &syn::ImplItemFn,
    args: ProcessArgs,
) -> syn::Result<TokenStream2> {
    let provider_name = provider.unraw().to_string();
    let ProcessArgs {
        queue,
        retries,
        concurrency,
        throttle,
        timeout,
        transactional,
    } = args;

    // The checkpoint parameter, when the method takes one: recognised by its
    // type and set aside, so the one job argument left is read by the shared
    // payload rule `#[on_event]` uses too.
    let checkpoint = checkpoint_parameter(method)?;
    if let Some(checkpoint) = &checkpoint
        && transactional != Some(false)
    {
        return Err(syn::Error::new(checkpoint.span, CHECKPOINT_NEEDS_THE_POOL));
    }
    let job_ty = {
        let mut signature = method.clone();
        if let Some(checkpoint) = &checkpoint {
            signature.sig.inputs = signature
                .sig
                .inputs
                .into_iter()
                .enumerate()
                .filter(|(index, _)| *index != checkpoint.index)
                .map(|(_, input)| input)
                .collect();
        }
        payload_arg_type(&signature, "#[process]", "job", provider)?
    };
    // A `Piped<P, T>` / `Valid<T>` job argument is a per-argument pipe: the
    // payload is `T`, the pipe runs after deserialization, and the handler
    // receives the carrier.
    let (deser_ty, job_wrap) = pipe_binding(&job_ty);

    let QueueId::Type(queue_ty) = &queue;
    let queue_str = quote!(<#queue_ty as ::nest_rs_queue::Queue>::NAME);
    let queue_assert = quote! {
        const _: () = {
            // Requires `<#queue_ty as Queue>::Job == #deser_ty`; a mismatch
            // fails here naming both the queue's `Job` and the handler's
            // argument type.
            fn __nestrs_assert_queue_job<__Q>()
            where
                __Q: ::nest_rs_queue::Queue<Job = #deser_ty>,
            {
            }
            let _ = __nestrs_assert_queue_job::<#queue_ty>;
        };
    };

    let method_ident = method.sig.ident.clone();
    // Un-raw: a label is read, and a generated identifier cannot hold `r#`.
    let method_name = method_ident.unraw().to_string();
    let qualified_name = format!("{provider_name}::{method_name}");
    let cfgs = cfg_attrs(&method.attrs);
    let handler_ident = format_ident!(
        "__nestrs_process_handler_{}_{}",
        snake_case(&provider_name),
        snake_case(&method_name)
    );

    let mut options = quote!(::nest_rs_queue::ProcessOptions::DEFAULT);
    if let Some(retries) = retries {
        options = quote!(#options.with_retries(#retries));
    }
    if let Some(concurrency) = concurrency {
        let above_one = concurrency - 1;
        options = quote! {
            #options.with_concurrency(::core::num::NonZeroU32::MIN.saturating_add(#above_one))
        };
    }
    if let Some(ThrottleArgs { limit, window_ms }) = throttle {
        let above_one = limit - 1;
        options = quote! {
            #options.with_throttle(::nest_rs_queue::Throttle::new(
                ::core::num::NonZeroU32::MIN.saturating_add(#above_one),
                ::std::time::Duration::from_millis(#window_ms),
            ))
        };
    }
    if checkpoint.is_some() {
        options = quote!(#options.with_checkpoint(true));
    }
    let timeout = job_timeout(timeout, &quote!(::nest_rs_queue));
    options = quote!(#options.with_timeout(#timeout));

    let transaction_tokens = job_transaction(transactional, &quote!(::nest_rs_queue));
    let checkpoint_open = checkpoint.as_ref().map(|checkpoint| {
        let state = &checkpoint.state;
        quote! {
            let __checkpoint = ::nest_rs_queue::Checkpoint::<#state>::open(
                __context.checkpoints.as_ref(),
                #queue_str,
            )
            .await?;
        }
    });
    // The method's arguments in the order it declares them.
    let call_args: Vec<TokenStream2> = method
        .sig
        .inputs
        .iter()
        .enumerate()
        .filter(|(_, input)| matches!(input, FnArg::Typed(_)))
        .map(|(index, _)| match &checkpoint {
            Some(checkpoint) if checkpoint.index == index => quote!(__checkpoint),
            _ => quote!(__job),
        })
        .collect();
    let call = await_if_async(
        &method.sig,
        quote!(<#self_ty>::#method_ident(&__provider, #(#call_args),*)),
    );

    Ok(quote! {
        #(#cfgs)*
        #queue_assert

        #(#cfgs)*
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #handler_ident(
            __payload: ::std::borrow::Cow<'_, ::nest_rs_queue::serde_json::Value>,
            __context: ::nest_rs_queue::HandlerContext,
        ) -> ::std::pin::Pin<
            ::std::boxed::Box<
                dyn ::std::future::Future<
                    Output = ::std::result::Result<(), ::nest_rs_queue::JobError>,
                > + ::std::marker::Send + '_,
            >,
        > {
            ::std::boxed::Box::pin(async move {
                let __deser: #deser_ty = match ::nest_rs_queue::decode(__payload) {
                    ::std::result::Result::Ok(j) => j,
                    ::std::result::Result::Err(e) => {
                        // Deterministic: the same bytes never deserialize on retry.
                        return ::std::result::Result::Err(
                            ::nest_rs_queue::JobError::undecodable(#queue_str, &e),
                        );
                    }
                };
                // Identity when the argument is a plain job type; runs the
                // pipe (surfacing a `PipeError` as the boxed job error) for a
                // `Piped<P, T>` / `Valid<T>` argument.
                let __job = #job_wrap;
                let __provider = match ::nest_rs_core::Container::get::<#self_ty>(&__context.container) {
                    ::std::option::Option::Some(p) => p,
                    ::std::option::Option::None => {
                        // Deterministic: a missing provider stays missing on
                        // retry — abort and dead-letter.
                        return ::std::result::Result::Err(
                            ::nest_rs_queue::JobError::abort(::std::format!(
                                "queue processor provider `{}` not registered in the running \
                                 container — add it to a reachable module's `providers = [...]`",
                                ::std::any::type_name::<#self_ty>(),
                            )),
                        );
                    }
                };
                #checkpoint_open
                let __job_context = ::nest_rs_core::Container::get_dyn::<
                    dyn ::nest_rs_queue::nest_rs_worker::JobContext,
                >(&__context.container);
                // The user `#[process]` method's `Err` is a transient fault —
                // retryable within the budget. Mapped *inside* the context so the
                // settling seam reads one error type and can report a commit it
                // could not honour in it.
                ::nest_rs_queue::nest_rs_worker::run_in_job_context(
                    __job_context.as_ref(),
                    #transaction_tokens,
                    async move {
                        #call.map_err(::nest_rs_queue::JobError::retry)
                    },
                    ::std::result::Result::is_ok,
                    // A job that ran fine but whose transaction could not be
                    // settled has written nothing, so the attempt fails rather
                    // than reporting a success that lost its writes. Whether it is
                    // *retried* is the context's call.
                    |__why| ::std::result::Result::Err(
                        ::nest_rs_queue::JobError::unhonoured(__why),
                    ),
                )
                .await
            })
        }

        #(#cfgs)*
        ::nest_rs_core::inventory::submit! {
            ::nest_rs_queue::ProcessMethod::new(
                ::core::module_path!(),
                #qualified_name,
                #queue_str,
                #options,
                || ::std::any::TypeId::of::<#self_ty>(),
                #handler_ident,
            )
        }
    })
}

/// A `Checkpoint<S>` parameter: where it sits among the method's inputs, the
/// state it saves, and where to point a refusal.
struct CheckpointParam {
    index: usize,
    state: Type,
    span: Span,
}

/// The method's `Checkpoint<S>` parameter, if it takes one — recognised by the
/// last segment of its type, so a qualified path works as a bare one does.
fn checkpoint_parameter(method: &syn::ImplItemFn) -> syn::Result<Option<CheckpointParam>> {
    let mut found: Option<CheckpointParam> = None;
    for (index, input) in method.sig.inputs.iter().enumerate() {
        let FnArg::Typed(typed) = input else {
            continue;
        };
        let Some((ident, types)) = generic_args(&typed.ty) else {
            continue;
        };
        if ident != "Checkpoint" {
            continue;
        }
        let [state] = types.as_slice() else {
            return Err(syn::Error::new_spanned(
                &typed.ty,
                "`Checkpoint` takes one type — the state it saves, `Checkpoint<ImportProgress>`",
            ));
        };
        if found.is_some() {
            return Err(syn::Error::new_spanned(
                &typed.ty,
                "a #[process] method takes at most one `Checkpoint<_>` parameter — a job keeps one \
                 checkpoint",
            ));
        }
        found = Some(CheckpointParam {
            index,
            state: (*state).clone(),
            span: typed.ty.span(),
        });
    }
    Ok(found)
}

/// `#[processor]` takes no arguments — the queues are named by the `#[process]`
/// methods it collects. `version` is called out first because it is the one key
/// a developer arrives with from `#[controller(version = "1")]`.
fn reject_args(args: TokenStream) -> syn::Result<()> {
    let args = TokenStream2::from(args);
    Edge::Queue.reject_version(&args)?;
    pair::PROCESSOR.reject_args(&args, "the provider's scope is declared by")
}

/// Split a job argument into (type to deserialize, expression yielding what the
/// handler receives). For a per-argument pipe `Piped<P, T>` / `Valid<T>` the wire
/// type is `T`, and the expression runs the pipe over `__deser`, surfacing a
/// `PipeError` as the queue's boxed error.
fn pipe_binding(job_ty: &Type) -> (Type, TokenStream2) {
    // A pipe rejection is deterministic (the same payload fails the pipe again),
    // so it aborts rather than retries.
    let box_err = quote! {
        |__e: ::nest_rs_pipes::PipeError| {
            // The message *and* the per-field detail.
            let __msg = __e.message().to_string();
            ::nest_rs_queue::JobError::abort(__msg).with_details(__e.into_details())
        }
    };
    match pipe_wrapper(job_ty) {
        Some(PipeWrapper::Piped { pipe, value }) => (
            value.clone(),
            quote! {
                ::nest_rs_pipes::Piped::<#pipe, #value>::apply(__deser).map_err(#box_err)?
            },
        ),
        Some(PipeWrapper::Valid { value }) => (
            value.clone(),
            quote! {
                ::nest_rs_pipes::Valid::<#value>::apply(__deser).map_err(#box_err)?
            },
        ),
        None => (job_ty.clone(), quote!(__deser)),
    }
}

/// How a `#[process]` names its queue: the `Queue` type its `#[queue]` marker
/// implements (`#[process(queue = AudioQueue)]`), which links the name and the
/// payload type to the one declaration at the feature port.
enum QueueId {
    // Boxed: `syn::Type` is a large enum (clippy::large_enum_variant).
    Type(Box<Type>),
}

impl Parse for QueueId {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(LitStr) {
            let lit: LitStr = input.parse()?;
            Err(syn::Error::new_spanned(
                &lit,
                takes_value(
                    "process",
                    Some("queue"),
                    &format!(
                        "the queue's `#[queue]` marker type, not a string: declare \
                         `#[queue(name = {:?}, job = <Payload>)] struct <Name>Queue;` at the \
                         feature port and write `#[process(queue = <Name>Queue)]` — the type \
                         form also checks this method's payload against the queue's",
                        lit.value(),
                    ),
                ),
            ))
        } else {
            input
                .parse::<Type>()
                .map(|marker| QueueId::Type(Box::new(marker)))
                .map_err(|stopped| {
                    syn::Error::new(
                        stopped.span(),
                        takes_value(
                            "process",
                            Some("queue"),
                            "the `#[queue]` marker type the method drains, e.g. \
                             `queue = AudioQueue`",
                        ),
                    )
                })
        }
    }
}

/// The refusal of a `#[process]` naming no queue — worded once for the empty
/// list and for the attribute written with no list at all.
fn missing_queue() -> String {
    format!(
        "{} (the `#[queue]` marker type the method drains)",
        missing_argument("process", "queue", "AudioQueue"),
    )
}

/// `throttle(limit = N, window = "…")`, read.
struct ThrottleArgs {
    limit: u32,
    window_ms: u64,
}

struct ProcessArgs {
    queue: QueueId,
    retries: Option<u32>,
    concurrency: Option<u32>,
    throttle: Option<ThrottleArgs>,
    /// The shared `timeout` key in milliseconds, `None` when unwritten.
    timeout: Option<u64>,
    /// The shared `transactional` key, `None` when unwritten — see
    /// `nest_rs_codegen::job`, which words it for every job decorator at once.
    transactional: Option<bool>,
}

impl Parse for ProcessArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut queue: Option<QueueId> = None;
        let mut retries: Option<u32> = None;
        let mut concurrency: Option<u32> = None;
        let mut throttle: Option<ThrottleArgs> = None;
        let mut timeout: Option<u64> = None;
        let mut transactional: Option<bool> = None;

        // The family's table answers first, refusing a key another member takes;
        // a key the table gains is a variant this match must place to compile.
        PROCESS.grammar().parse(input, |arg| {
            match job_key(PROCESS, &arg)? {
                JobKey::Queue => queue = Some(arg.value()?.parse()?),
                JobKey::Retries => {
                    retries = Some(whole_number(
                        &arg.expr()?,
                        &format!(
                            "{} — the re-runs a failed attempt gets before the job dead-letters",
                            takes_value("process", Some(arg.key()), "a whole number"),
                        ),
                    )?);
                }
                JobKey::Concurrency => {
                    concurrency = Some(at_least_one(
                        &arg.expr()?,
                        &format!(
                            "{} — how many jobs of this method one worker replica runs at once",
                            takes_value("process", Some(arg.key()), AT_LEAST_ONE),
                        ),
                    )?);
                }
                JobKey::Throttle => {
                    let input = arg.input();
                    if !input.peek(syn::token::Paren) {
                        return Err(syn::Error::new(
                            arg.ident().span(),
                            format!(
                                "{} — write `{}`",
                                takes_value("process", Some(arg.key()), "a list"),
                                JobKey::Throttle.example(),
                            ),
                        ));
                    }
                    let content;
                    syn::parenthesized!(content in input);
                    throttle = Some(parse_throttle(&content, arg.ident().span())?);
                }
                JobKey::Timeout => timeout = Some(timeout_value(PROCESS, &arg.expr()?)?),
                JobKey::Transactional => {
                    transactional = Some(transactional_value(PROCESS, &arg.expr()?)?);
                }
                unread @ (JobKey::Tz | JobKey::Replicas | JobKey::Key) => {
                    return Err(unread_job_key(PROCESS, unread, arg.ident()));
                }
            }
            Ok(())
        })?;

        let queue = queue.ok_or_else(|| syn::Error::new(input.span(), missing_queue()))?;

        Ok(Self {
            queue,
            retries,
            concurrency,
            throttle,
            timeout,
            transactional,
        })
    }
}

/// The keys inside `throttle(..)`: both required, each once.
fn parse_throttle(content: ParseStream, at: Span) -> syn::Result<ThrottleArgs> {
    let mut limit: Option<u32> = None;
    let mut window_ms: Option<u64> = None;
    THROTTLE.parse(content, |arg| {
        let value = arg.expr()?;
        if arg.key() == "limit" {
            limit = Some(at_least_one(
                &value,
                &format!(
                    "{} — how many jobs may start in one window",
                    takes_value("process", Some("throttle(limit)"), AT_LEAST_ONE),
                ),
            )?);
        } else {
            window_ms = Some(duration_millis(
                "process",
                Some("throttle(window)"),
                &value,
            )?);
        }
        Ok(())
    })?;
    match (limit, window_ms) {
        (Some(limit), Some(window_ms)) => Ok(ThrottleArgs { limit, window_ms }),
        _ => Err(syn::Error::new(
            at,
            format!(
                "#[process] `throttle` needs both `limit` and `window` — write `{}`",
                JobKey::Throttle.example(),
            ),
        )),
    }
}

/// A whole-number literal, or `refusal` spanned at what was written instead.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
fn whole_number(expr: &Expr, refusal: &str) -> syn::Result<u32> {
    match ungrouped_expr(expr) {
        Expr::Lit(ExprLit {
            lit: Lit::Int(int), ..
        }) => int
            .base10_parse::<u32>()
            .map_err(|_| syn::Error::new_spanned(int, refusal)),
        other => Err(syn::Error::new_spanned(other, refusal)),
    }
}

/// What a count key takes, in the words both of them refuse with.
const AT_LEAST_ONE: &str = "a whole number of at least 1";

/// A whole-number literal of at least one — zero is refused naming why.
fn at_least_one(expr: &Expr, refusal: &str) -> syn::Result<u32> {
    let value = whole_number(expr, refusal)?;
    if value == 0 {
        return Err(syn::Error::new_spanned(
            ungrouped_expr(expr),
            format!("{refusal}; 0 would never run a job"),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key the job-key table gives `#[process]` is read by this parser,
    /// written as the table's own example — which is also what the sentences
    /// offer a developer to paste, so each offered spelling is proved to parse.
    #[test]
    fn every_key_of_the_column_is_read() {
        for key in nest_rs_codegen::job_keys(PROCESS) {
            let written: TokenStream2 = key.example().parse().expect("an example is tokens");
            let args = match key {
                JobKey::Queue => written,
                _ => {
                    let queue: TokenStream2 = JobKey::Queue.example().parse().expect("tokens");
                    quote!(#queue, #written)
                }
            };
            if let Err(refusal) = syn::parse2::<ProcessArgs>(args) {
                panic!("#[process] does not read `{}`: {refusal}", key.name());
            }
        }
    }
}
