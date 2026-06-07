//! `#[processor]` — orchestrator on a provider's `impl` block. Walks the
//! methods; for each one tagged with `#[process(queue = …, concurrency, retries)]`
//! emits a type-erased handler `fn` and a `ProcessMethod` inventory submission
//! the active queue backend (e.g. Redis via `nestrs-redis`) drains at boot.
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
//! backend integration (nestrs-redis, …) is wired in.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    FnArg, Ident, ImplItem, ItemImpl, LitInt, LitStr, PatType, Token, Type, parse_macro_input,
};

pub(crate) fn processor(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemImpl);
    let self_ty = item.self_ty.clone();
    let provider_ident = match impl_self_ident(&self_ty) {
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
            concurrency,
            retries,
        } = args;

        let job_ty = match extract_job_type(method) {
            Ok(ty) => ty,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_ident = method.sig.ident.clone();
        let method_name = method_ident.to_string();
        let qualified_name = format!("{provider_name}::{method_name}");

        let provider_snake = to_snake(&provider_name);
        let method_snake = to_snake(&method_name);
        let handler_ident = format_ident!(
            "__nestrs_process_handler_{}_{}",
            provider_snake,
            method_snake
        );

        let concurrency_lit = LitInt::new(&concurrency.to_string(), proc_macro2::Span::call_site());
        let retries_lit = LitInt::new(&retries.to_string(), proc_macro2::Span::call_site());

        emissions.push(quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #handler_ident(
                __payload: ::nest_rs_queue::serde_json::Value,
                __container: ::nest_rs_core::Container,
            ) -> ::std::pin::Pin<
                ::std::boxed::Box<
                    dyn ::std::future::Future<
                        Output = ::std::result::Result<
                            (),
                            ::std::boxed::Box<
                                dyn ::std::error::Error + ::std::marker::Send + ::std::marker::Sync,
                            >,
                        >,
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
                                    #queue,
                                )
                            } else {
                                ::std::format!(
                                    "unsupported job wire-format version {0} on queue `{1}`; \
                                     the producer is from an older release; either drain \
                                     the queue or pin the consumer at version {0}",
                                    __v,
                                    #queue,
                                )
                            };
                            return ::std::result::Result::Err(::std::boxed::Box::<
                                dyn ::std::error::Error + ::std::marker::Send + ::std::marker::Sync,
                            >::from(__msg));
                        }
                        __obj.remove("payload").unwrap_or(
                            ::nest_rs_queue::serde_json::Value::Null,
                        )
                    } else {
                        ::nest_rs_queue::tracing::warn!(
                            target: "nest_rs::queue",
                            queue = #queue,
                            "processed an unversioned job payload — producer predates the \
                             wire envelope; drain the queue to clear legacy jobs",
                        );
                        __payload
                    };
                    let __job: #job_ty = match ::nest_rs_queue::serde_json::from_value(__raw) {
                        ::std::result::Result::Ok(j) => j,
                        ::std::result::Result::Err(e) => {
                            return ::std::result::Result::Err(::std::boxed::Box::<
                                dyn ::std::error::Error + ::std::marker::Send + ::std::marker::Sync,
                            >::from(::std::format!(
                                "failed to deserialize job for queue `{}`: {e}",
                                #queue,
                            )));
                        }
                    };
                    let __provider = match ::nest_rs_core::Container::get::<#self_ty>(&__container) {
                        ::std::option::Option::Some(p) => p,
                        ::std::option::Option::None => {
                            return ::std::result::Result::Err(::std::boxed::Box::<
                                dyn ::std::error::Error + ::std::marker::Send + ::std::marker::Sync,
                            >::from(::std::format!(
                                "queue processor provider `{}` not registered in the running \
                                 container — add it to a reachable module's `providers = [...]`",
                                ::std::any::type_name::<#self_ty>(),
                            )));
                        }
                    };
                    let __job_context = ::nest_rs_core::Container::get_dyn::<
                        dyn ::nest_rs_worker::JobContext,
                    >(&__container);
                    ::nest_rs_worker::run_in_job_context(
                        __job_context.as_ref(),
                        async move { <#self_ty>::#method_ident(&__provider, __job).await },
                    )
                    .await
                    .map_err(::std::convert::Into::into)
                })
            }

            ::nest_rs_core::inventory::submit! {
                ::nest_rs_queue::ProcessMethod {
                    name: #qualified_name,
                    queue: #queue,
                    concurrency: #concurrency_lit,
                    retries: #retries_lit,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    handler: #handler_ident,
                }
            }
        });
    }

    let out = quote! {
        #item
        #(#emissions)*
    };
    out.into()
}

fn impl_self_ident(self_ty: &Type) -> syn::Result<Ident> {
    if let Type::Path(p) = self_ty
        && let Some(seg) = p.path.segments.last()
    {
        return Ok(seg.ident.clone());
    }
    Err(syn::Error::new_spanned(
        self_ty,
        "#[processor] expects an `impl` block on a named struct (e.g. `impl AudioJobs`)",
    ))
}

/// Extract the second parameter's type — the job payload. Errors out crisply
/// when the signature is wrong (no `&self`, or no job arg).
fn extract_job_type(method: &syn::ImplItemFn) -> syn::Result<Type> {
    let mut iter = method.sig.inputs.iter();
    match iter.next() {
        Some(FnArg::Receiver(_)) => {}
        Some(other) => {
            return Err(syn::Error::new(
                other.span(),
                "a `#[process]` method must take `&self` as its first argument",
            ));
        }
        None => {
            return Err(syn::Error::new(
                method.sig.span(),
                "a `#[process]` method must take `&self` and one job argument",
            ));
        }
    }
    let Some(arg) = iter.next() else {
        return Err(syn::Error::new(
            method.sig.span(),
            "a `#[process]` method needs a job argument: `async fn(&self, job: T)`",
        ));
    };
    match arg {
        FnArg::Typed(PatType { ty, .. }) => Ok((**ty).clone()),
        FnArg::Receiver(r) => Err(syn::Error::new(
            r.span(),
            "a `#[process]` method takes exactly one `&self` receiver",
        )),
    }
}

struct ProcessArgs {
    queue: LitStr,
    concurrency: usize,
    retries: usize,
}

impl Parse for ProcessArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut queue: Option<LitStr> = None;
        let mut concurrency: usize = 1;
        let mut retries: usize = 0;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "queue" => queue = Some(input.parse()?),
                "concurrency" => concurrency = input.parse::<LitInt>()?.base10_parse()?,
                "retries" => retries = input.parse::<LitInt>()?.base10_parse()?,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown #[process] key `{other}` \
                             (expected `queue`, `concurrency`, or `retries`)"
                        ),
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
                "#[process] requires a `queue = \"...\"` argument",
            )
        })?;

        Ok(Self {
            queue,
            concurrency,
            retries,
        })
    }
}

fn to_snake(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for (i, ch) in camel.chars().enumerate() {
        if ch.is_uppercase() && i != 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}
