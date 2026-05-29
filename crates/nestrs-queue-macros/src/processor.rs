//! `#[processor]` implementation: parse the queue args, then emit construction +
//! `Discoverable` attaching a `ProcessorMeta`.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Ident, ItemStruct, LitInt, LitStr, Token};

use nestrs_codegen::{build_injectable_body, injected_method, InjectableBody};

pub(crate) fn processor(args: TokenStream, input: TokenStream) -> TokenStream {
    let ProcessorArgs {
        queue,
        concurrency,
        retries,
    } = match syn::parse::<ProcessorArgs>(args) {
        Ok(args) => args,
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
    let injected = injected_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics ::nestrs_queue::FromContainer for #name #ty_generics #where_clause {
            fn from_container(container: &::nestrs_core::Container) -> Self {
                let _ = container;
                #ctor
            }
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #injected

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                builder.attach_meta::<Self, ::nestrs_queue::ProcessorMeta>(
                    ::nestrs_queue::ProcessorMeta {
                        name: #name_lit,
                        queue: #queue,
                        concurrency: #concurrency,
                        retries: #retries,
                        register: ::nestrs_queue::register_worker::<Self>,
                    },
                )
            }
        }
    }
    .into()
}

/// Parsed `#[processor(...)]` arguments: `queue = "..."` (required),
/// `concurrency = N` (default 1), `retries = N` (default 0), in any order.
struct ProcessorArgs {
    queue: LitStr,
    concurrency: usize,
    retries: usize,
}

impl Parse for ProcessorArgs {
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
                            "unknown #[processor] key `{other}` \
                             (expected `queue`, `concurrency`, or `retries`)"
                        ),
                    ))
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        let queue = queue.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                "#[processor] requires a `queue = \"...\"` argument",
            )
        })?;

        Ok(Self {
            queue,
            concurrency,
            retries,
        })
    }
}
