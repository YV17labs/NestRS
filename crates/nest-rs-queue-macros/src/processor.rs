//! `#[processor]` — orchestrator on a provider's `impl` block. Walks the
//! methods; for each one tagged with `#[process(queue = …, retries)]`
//! emits a type-erased handler `fn` and a `ProcessMethod` inventory submission
//! the active queue backend (e.g. Redis via `nest-rs-redis`) drains at boot.
//!
//! Like `#[scheduled]`, this does NOT emit `Discoverable` for the host
//! struct — the user's own `#[injectable]` owns it. Inventory is the seam.
//!
//! The handler is emitted as a `nest_rs_queue::JobHandler` — a fn pointer
//! that takes the raw JSON payload + a `Container`, deserializes to the
//! method's job type, resolves the provider, and dispatches. Every reference
//! is to `::nest_rs_queue::*` (the abstractions crate, which also re-exports
//! this macro and `serde_json`), so the call site reaches the macro and the
//! emission targets through the same import root regardless of which
//! backend integration (nest-rs-redis, …) is wired in.

use nest_rs_codegen::{
    DecoratorPair, Edge, PipeWrapper, TRANSACTIONAL, duplicate_argument, impl_self_ident,
    job_argument_needs_a_value, job_transaction, payload_arg_type, pipe_wrapper, snake_case,
    transactional_value, unknown_argument,
};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Ident, ImplItem, LitInt, LitStr, Token, Type};

/// A queue processor has no edge struct decorator — the host keeps its own
/// `#[injectable]` — but reaching for `#[processor]` on the struct still deserves
/// a sentence naming what to write instead of syn's `expected impl`.
const PROCESSOR_PAIR: DecoratorPair = DecoratorPair::on_provider("#[processor]", "#[process]");

pub(crate) fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    if let Err(err) = reject_args(args) {
        return err.to_compile_error().into();
    }

    let mut item = match PROCESSOR_PAIR.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let host_check = PROCESSOR_PAIR.provider_host_check(&self_ty);
    let provider_ident = match impl_self_ident(&self_ty, "#[processor]") {
        Ok(ident) => ident,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_name = provider_ident.to_string();

    let mut emissions: Vec<TokenStream2> = Vec::new();

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let attr_idx = method
            .attrs
            .iter()
            .position(|attr| attr.path().is_ident("process"));
        let Some(idx) = attr_idx else { continue };
        let attr = method.attrs.remove(idx);

        let args = match attr.parse_args::<ProcessArgs>() {
            Ok(a) => a,
            Err(err) => return err.to_compile_error().into(),
        };
        let ProcessArgs {
            queue,
            retries,
            transactional,
        } = args;

        let job_ty = match payload_arg_type(method, "#[process]", "job") {
            Ok(ty) => ty,
            Err(err) => return err.to_compile_error().into(),
        };
        // A `Piped<P, T>` / `Valid<T>` job argument is a per-argument pipe: the
        // wire payload is `T`, the pipe runs after deserialization, and the
        // handler receives the carrier. Matches the HTTP / GraphQL forms.
        let (deser_ty, job_wrap) = pipe_binding(&job_ty);

        // The queue is named by its `QueueName` type
        // (`#[process(queue = AudioQueue)]`), which yields
        // `<Q as QueueName>::NAME` — a `&'static str` const — and additionally
        // asserts, at compile time, that the method's wire payload is exactly
        // `<Q as QueueName>::Job`. A mismatch is an error naming both types.
        let queue_str: TokenStream2 = match &queue {
            QueueId::Type(ty) => {
                let ty: &Type = ty;
                quote!(<#ty as ::nest_rs_queue::QueueName>::NAME)
            }
        };
        let queue_assert: TokenStream2 = match &queue {
            QueueId::Type(ty) => {
                let ty: &Type = ty;
                quote! {
                const _: () = {
                    // Requires `<#ty as QueueName>::Job == #deser_ty`; a
                    // mismatch fails here naming both the queue's `Job` and the
                    // handler's argument type.
                    fn __nestrs_assert_queue_job<__Q>()
                    where
                        __Q: ::nest_rs_queue::QueueName<Job = #deser_ty>,
                    {
                    }
                    let _ = __nestrs_assert_queue_job::<#ty>;
                };
                }
            }
        };

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.to_string();
        let qualified_name = format!("{provider_name}::{method_name}");

        let provider_snake = snake_case(&provider_name);
        let method_snake = snake_case(&method_name);
        let handler_ident = format_ident!(
            "__nestrs_process_handler_{}_{}",
            provider_snake,
            method_snake
        );

        let retries_lit = LitInt::new(&retries.to_string(), proc_macro2::Span::call_site());
        let transaction_tokens = job_transaction(transactional, &quote!(::nest_rs_queue));

        emissions.push(quote! {
            #queue_assert

            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #handler_ident(
                __payload: ::nest_rs_queue::serde_json::Value,
                __container: ::nest_rs_core::Container,
            ) -> ::std::pin::Pin<
                ::std::boxed::Box<
                    dyn ::std::future::Future<
                        Output = ::std::result::Result<(), ::nest_rs_queue::JobError>,
                    > + ::std::marker::Send,
                >,
            > {
                ::std::boxed::Box::pin(async move {
                    // Unwrap the wire envelope `{ "v": <n>, "payload": <…> }`.
                    // Detection is strict to avoid mis-classifying a user `Job`
                    // struct that happens to have `v`+`payload` fields plus
                    // anything else:
                    //   - the object MUST have exactly two top-level keys, and
                    //     they MUST be `v` and `payload`;
                    //   - `v` MUST be a JSON Number with a non-negative integer
                    //     value (accepting both `1` and `1.0` — a hand-rolled
                    //     producer may serialize as a float).
                    // Anything else falls through to the legacy raw-decode path
                    // (with a warning), so jobs left in Redis from a prior
                    // deploy still drain.
                    let __is_envelope = match &__payload {
                        ::nest_rs_queue::serde_json::Value::Object(__obj) => {
                            __obj.len() == 2
                                && __obj.contains_key("v")
                                && __obj.contains_key("payload")
                                && match __obj.get("v") {
                                    ::std::option::Option::Some(
                                        ::nest_rs_queue::serde_json::Value::Number(__n),
                                    ) => {
                                        __n.as_u64().is_some()
                                            || __n.as_f64().is_some_and(|__f| {
                                                __f.is_finite()
                                                    && __f >= 0.0
                                                    && __f.fract() == 0.0
                                            })
                                    }
                                    _ => false,
                                }
                        }
                        _ => false,
                    };
                    let __raw: ::nest_rs_queue::serde_json::Value = if __is_envelope {
                        let ::nest_rs_queue::serde_json::Value::Object(mut __obj) = __payload else {
                            ::std::unreachable!("__is_envelope guarantees an Object");
                        };
                        let __v_value = __obj.remove("v").unwrap_or(
                            ::nest_rs_queue::serde_json::Value::Null,
                        );
                        let __v = match &__v_value {
                            ::nest_rs_queue::serde_json::Value::Number(__n) => __n
                                .as_u64()
                                .or_else(|| __n.as_f64().map(|__f| __f as u64))
                                .unwrap_or(u64::MAX),
                            _ => u64::MAX,
                        };
                        if __v != ::nest_rs_queue::WIRE_FORMAT_VERSION as u64 {
                            let __msg = if __v > ::nest_rs_queue::WIRE_FORMAT_VERSION as u64 {
                                ::std::format!(
                                    "unsupported job wire-format version {} on queue `{}`; \
                                     the producer is from a newer release; either roll back \
                                     this consumer or wait for the producer to drain",
                                    __v,
                                    #queue_str,
                                )
                            } else {
                                ::std::format!(
                                    "unsupported job wire-format version {0} on queue `{1}`; \
                                     the producer is from an older release; either drain \
                                     the queue or pin the consumer at version {0}",
                                    __v,
                                    #queue_str,
                                )
                            };
                            // Deterministic: a wrong wire version never succeeds
                            // on retry — abort and dead-letter (QUEUE-I4).
                            return ::std::result::Result::Err(
                                ::nest_rs_queue::JobError::abort(__msg),
                            );
                        }
                        __obj.remove("payload").unwrap_or(
                            ::nest_rs_queue::serde_json::Value::Null,
                        )
                    } else {
                        ::nest_rs_queue::tracing::warn!(
                            target: ::nest_rs_queue::TARGET,
                            queue = #queue_str,
                            hint = "producer predates the wire envelope; drain the queue to clear legacy jobs",
                            "processed an unversioned job payload",
                        );
                        __payload
                    };
                    let __deser: #deser_ty = match ::nest_rs_queue::serde_json::from_value(__raw) {
                        ::std::result::Result::Ok(j) => j,
                        ::std::result::Result::Err(e) => {
                            // Deterministic: the same bytes never deserialize on
                            // retry — abort and dead-letter (QUEUE-I4).
                            return ::std::result::Result::Err(
                                ::nest_rs_queue::JobError::abort(::std::format!(
                                    "failed to deserialize job for queue `{}`: {e}",
                                    #queue_str,
                                )),
                            );
                        }
                    };
                    // Identity when the argument is a plain job type; runs the
                    // pipe (surfacing a `PipeError` as the boxed job error) for a
                    // `Piped<P, T>` / `Valid<T>` argument.
                    let __job = #job_wrap;
                    let __provider = match ::nest_rs_core::Container::get::<#self_ty>(&__container) {
                        ::std::option::Option::Some(p) => p,
                        ::std::option::Option::None => {
                            // Deterministic: a missing provider stays missing on
                            // retry — abort and dead-letter (QUEUE-I4).
                            return ::std::result::Result::Err(
                                ::nest_rs_queue::JobError::abort(::std::format!(
                                    "queue processor provider `{}` not registered in the running \
                                     container — add it to a reachable module's `providers = [...]`",
                                    ::std::any::type_name::<#self_ty>(),
                                )),
                            );
                        }
                    };
                    let __job_context = ::nest_rs_core::Container::get_dyn::<
                        dyn ::nest_rs_queue::nest_rs_worker::JobContext,
                    >(&__container);
                    // The user `#[process]` method's `Err` is a transient fault —
                    // retryable (the backend's retry budget applies). Mapped
                    // *inside* the context so the settling seam reads one error
                    // type and can report a commit it could not honour in it.
                    ::nest_rs_queue::nest_rs_worker::run_in_job_context(
                        __job_context.as_ref(),
                        #transaction_tokens,
                        async move {
                            <#self_ty>::#method_ident(&__provider, __job)
                                .await
                                .map_err(|__e| ::nest_rs_queue::JobError::retry(__e))
                        },
                        ::std::result::Result::is_ok,
                        // A job that ran fine but whose transaction could not be
                        // settled has written nothing, so the attempt fails
                        // rather than reporting a success that lost its writes.
                        // Whether it is *retried* is the context's call: a
                        // deterministic failure re-fails identically, having
                        // replayed every side effect the body performs outside
                        // the transaction.
                        |__why| ::std::result::Result::Err(
                            ::nest_rs_queue::JobError::unhonoured(__why),
                        ),
                    )
                    .await
                })
            }

            ::nest_rs_core::inventory::submit! {
                ::nest_rs_queue::ProcessMethod {
                    origin: ::core::module_path!(),
                    name: #qualified_name,
                    queue: #queue_str,
                    retries: #retries_lit,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    handler: #handler_ident,
                }
            }
        });
    }

    let out = quote! {
        #item

        #host_check
        #(#emissions)*
    };
    out.into()
}

/// `#[processor]` takes no arguments — the queues are named by the `#[process]`
/// methods it collects. It used to *ignore* whatever it was handed, so anything
/// written here bound nothing and said nothing; `version` is called out first
/// because it is the one key a developer arrives with from
/// `#[controller(version = "1")]`, and "takes no arguments" answers a question
/// they did not ask.
fn reject_args(args: TokenStream) -> syn::Result<()> {
    let args = TokenStream2::from(args);
    Edge::Queue.reject_version(&args)?;
    PROCESSOR_PAIR.reject_args(&args, "the provider's scope is declared by")
}

/// Split a job argument into (type to deserialize from the wire, expression that
/// yields the value the handler receives). For a plain type both are trivial:
/// deserialize `T`, hand over `__deser`. For a per-argument pipe `Piped<P, T>` /
/// `Valid<T>` the wire type is `T`, and the expression runs the pipe over
/// `__deser`, surfacing a `PipeError` as the queue's boxed error.
fn pipe_binding(job_ty: &Type) -> (Type, TokenStream2) {
    // A pipe rejection is deterministic (the same payload fails the pipe again),
    // so it aborts rather than retries (QUEUE-I4).
    let box_err = quote! {
        |__e: ::nest_rs_pipes::PipeError| {
            // The message *and* the per-field detail: a dead-lettered job is
            // read from a log by someone who cannot re-run it, so
            // `error=validation failed` on its own throws away what the
            // rejection knew.
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

/// How a `#[process]` names its queue: a `QueueName` type path
/// (`#[process(queue = AudioQueue)]`) that links the wire name and the payload
/// type to the shared handle declared at the feature port.
///
/// A bare string used to be accepted too. It is gone: it named the queue
/// without naming its payload, so a consumer could deserialize a type the
/// producer never sends and the job would simply never drain — the typed form
/// turns exactly that into a compile error, which makes the string form a
/// strictly worse second way to say the same thing.
enum QueueId {
    // Boxed: `syn::Type` is a large enum, so an unboxed variant would bloat
    // every `QueueId` to its size (clippy::large_enum_variant).
    Type(Box<Type>),
}

impl Parse for QueueId {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(LitStr) {
            let lit: LitStr = input.parse()?;
            Err(syn::Error::new_spanned(
                &lit,
                format!(
                    "name the queue by its `QueueName` type, not a string: declare \
                     `#[queue(name = {:?}, job = <Payload>)] struct <Name>Queue;` at the \
                     feature port and write `#[process(queue = <Name>Queue)]` — the type \
                     form also checks this method's payload against the queue's",
                    lit.value(),
                ),
            ))
        } else {
            Ok(QueueId::Type(Box::new(input.parse()?)))
        }
    }
}

struct ProcessArgs {
    queue: QueueId,
    retries: usize,
    /// The shared `transactional` key, `None` when unwritten — see
    /// `nest_rs_codegen::job`, which words it for every job decorator at once.
    transactional: Option<bool>,
}

impl Parse for ProcessArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut queue: Option<QueueId> = None;
        let mut retries: Option<usize> = None;
        let mut transactional: Option<bool> = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let name = key.to_string();
            // The `=` is checked before it is consumed so a bare key earns a
            // sentence naming the key rather than syn's `expected `=``, which
            // names the grammar. Same refusal as the triggers', worded once.
            if !input.peek(Token![=]) {
                return Err(syn::Error::new(
                    key.span(),
                    job_argument_needs_a_value("process", &name),
                ));
            }
            input.parse::<Token![=]>()?;
            // A repeated key is refused, not last-write-wins — the same reading
            // `#[every]`/`#[cron]`/`#[after]` take, through the same sentence.
            // Which of two declarations gets dropped would be source order, and
            // one of the two it can drop is `transactional = true`: the default
            // this decorator exists to let a developer state.
            let taken = match name.as_str() {
                "queue" => queue.is_some(),
                "retries" => retries.is_some(),
                TRANSACTIONAL => transactional.is_some(),
                _ => false,
            };
            if taken {
                return Err(syn::Error::new(
                    key.span(),
                    duplicate_argument("process", &name),
                ));
            }
            match name.as_str() {
                "queue" => queue = Some(input.parse()?),
                "retries" => retries = Some(input.parse::<LitInt>()?.base10_parse()?),
                TRANSACTIONAL => transactional = Some(transactional_value(&input.parse()?)?),
                // `concurrency` was a real key. It is gone rather than
                // deprecated, so say what replaced it instead of listing the
                // survivors: the removal is a behaviour change, and a bare
                // "unknown key" would read as a typo.
                "concurrency" => {
                    return Err(syn::Error::new(
                        key.span(),
                        "`concurrency` is gone from #[process]: a process method runs one job \
                         at a time, and throughput scales by running more worker replicas — \
                         the unit the container platform already schedules. Drop the argument",
                    ));
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        // Built from the constant and through the shared
                        // sentence, so the four job decorators word an unknown
                        // key one way — and so a rename of the key cannot leave
                        // a literal behind in the file that already imports it.
                        unknown_argument("process", other, &["queue", "retries", TRANSACTIONAL]),
                    ));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        let queue = queue.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                "#[process] requires a `queue = \"...\"` (or `queue = <QueueName type>`) argument",
            )
        })?;

        Ok(Self {
            queue,
            retries: retries.unwrap_or(0),
            transactional,
        })
    }
}
