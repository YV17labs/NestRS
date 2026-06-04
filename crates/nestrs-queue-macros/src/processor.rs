//! `#[processor]` — orchestrator on a provider's `impl` block. Walks the
//! methods; for each one tagged with `#[process(queue = …, concurrency, retries)]`
//! emits a free handler `fn`, a monomorphic `register` thunk, and a
//! `ProcessMethod` inventory submission the `QueueWorker` drains at boot.
//!
//! Like `#[scheduled]`, this does NOT emit `Discoverable` for the host
//! struct — the user's own `#[injectable]` owns it. Inventory is the seam.

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
        let register_ident = format_ident!(
            "__nestrs_process_register_{}_{}",
            provider_snake,
            method_snake
        );

        let concurrency_lit = LitInt::new(&concurrency.to_string(), proc_macro2::Span::call_site());
        let retries_lit = LitInt::new(&retries.to_string(), proc_macro2::Span::call_site());

        emissions.push(quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #handler_ident(
                __job: #job_ty,
                __container: ::nestrs_queue::Data<::nestrs_core::Container>,
                __ctx: ::nestrs_queue::Data<
                    ::std::option::Option<::std::sync::Arc<dyn ::nestrs_core::JobContext>>,
                >,
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
                    let __provider = ::nestrs_core::Container::get::<#self_ty>(&__container)
                        .expect(::std::concat!(
                            "queue processor provider `",
                            #provider_name,
                            "` is not registered — add it to a reachable module's \
                             `providers = [...]`",
                        ));
                    let __ctx_ref: &::std::option::Option<
                        ::std::sync::Arc<dyn ::nestrs_core::JobContext>,
                    > = &__ctx;
                    ::nestrs_core::run_in_job_context(
                        __ctx_ref.as_ref(),
                        async move { <#self_ty>::#method_ident(&__provider, __job).await },
                    )
                    .await
                    .map_err(::std::convert::Into::into)
                })
            }

            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #register_ident(
                __monitor: ::nestrs_queue::Monitor,
                __conn: ::nestrs_queue::QueueConnection,
                __container: ::nestrs_core::Container,
                __meta: &::nestrs_queue::ProcessorMeta,
            ) -> ::nestrs_queue::Monitor {
                ::nestrs_queue::register_method::<#job_ty>(
                    __monitor,
                    __conn,
                    __container,
                    __meta,
                    #handler_ident,
                )
            }

            ::nestrs_core::inventory::submit! {
                ::nestrs_queue::ProcessMethod {
                    name: #qualified_name,
                    queue: #queue,
                    concurrency: #concurrency_lit,
                    retries: #retries_lit,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    register: #register_ident,
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
