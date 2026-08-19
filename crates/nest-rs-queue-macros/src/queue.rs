//! `#[queue(name = "…", job = Payload)]` — the tiny derive-style attribute that
//! stamps a unit struct with an `impl QueueName`, giving a queue a compile-time
//! identity (its wire name + its job type). Emits absolute `::nest_rs_queue::*`
//! paths so the macros crate never depends on its surface crate.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Ident, ItemStruct, LitStr, Token, Type, parse_macro_input};

pub(crate) fn queue(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as QueueArgs);
    let item = parse_macro_input!(input as ItemStruct);

    if !item.fields.is_empty() {
        return syn::Error::new(
            item.fields.span(),
            "#[queue] applies to a unit struct — the queue identity carries no \
             data (e.g. `#[queue(name = \"audio\", job = TranscodeCommand)] \
             pub struct AudioQueue;`)",
        )
        .to_compile_error()
        .into();
    }

    let ident = &item.ident;
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let QueueArgs { name, job } = args;

    let out = quote! {
        #item

        impl #impl_generics ::nest_rs_queue::QueueName for #ident #ty_generics #where_clause {
            const NAME: &'static str = #name;
            type Job = #job;
        }
    };
    out.into()
}

struct QueueArgs {
    name: LitStr,
    job: Type,
}

impl Parse for QueueArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name: Option<LitStr> = None;
        let mut job: Option<Type> = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            // The key is judged before the `=`, so a bare `#[queue(name)]`
            // names the key rather than dying on syn's `` expected `=` `` — and
            // a bare *unknown* key still reads as unknown rather than as
            // missing a value.
            let spelled = key.to_string();
            if !matches!(spelled.as_str(), "name" | "job") {
                return Err(syn::Error::new(
                    key.span(),
                    nest_rs_codegen::unknown_argument("queue", &spelled, &["name", "job"]),
                ));
            }
            if !input.peek(Token![=]) {
                return Err(syn::Error::new(
                    key.span(),
                    nest_rs_codegen::needs_a_value("queue", &spelled),
                ));
            }
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" => {
                    nest_rs_codegen::once(name.is_some(), &key, "queue", &key.to_string())?;
                    name = Some(input.parse()?);
                }
                "job" => {
                    nest_rs_codegen::once(job.is_some(), &key, "queue", &key.to_string())?;
                    job = Some(input.parse()?);
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        nest_rs_codegen::unknown_argument("queue", other, &["name", "job"]),
                    ));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        let name = name.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                "#[queue] requires a `name = \"...\"` argument",
            )
        })?;
        let job = job.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                "#[queue] requires a `job = <PayloadType>` argument naming the queue's payload",
            )
        })?;

        Ok(Self { name, job })
    }
}
