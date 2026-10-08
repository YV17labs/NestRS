//! `#[queue(name = "…", job = Payload)]` — the attribute that gives a unit
//! struct a queue's compile-time identity: its wire name and its payload type.
//! Emits absolute `::nest_rs_queue::*` paths so the macros crate never depends
//! on its surface crate.

use nest_rs_codegen::{
    Grammar, invalid_queue_name, is_valid_queue_name, missing_argument, takes_value,
};
use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Item, LitStr, Type, parse_macro_input};

/// Every key `#[queue]` takes, in the order its unknown-key refusal lists them.
const QUEUE: Grammar = Grammar::new("queue", &["name", "job"]);

pub(crate) fn queue(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as QueueArgs);
    let item = match parse_macro_input!(input as Item) {
        Item::Struct(item) if item.fields.is_empty() => item,
        Item::Struct(item) => {
            return syn::Error::new(
                item.fields.span(),
                not_a_unit_struct("a struct with fields"),
            )
            .to_compile_error()
            .into();
        }
        other => {
            // Name the shape written, rather than syn's `expected struct`.
            let shape = match &other {
                Item::Enum(_) => "an enum",
                Item::Union(_) => "a union",
                Item::Impl(_) => "an impl block",
                Item::Type(_) => "a type alias",
                Item::Trait(_) => "a trait",
                Item::Fn(_) => "a function",
                _ => "not a struct",
            };
            return syn::Error::new_spanned(&other, not_a_unit_struct(shape))
                .to_compile_error()
                .into();
        }
    };

    // A unit struct *can* carry a const parameter, and `Q<1>` and `Q<2>` would
    // then share one wire name.
    if let Some(parameter) = item.generics.params.first() {
        return syn::Error::new(
            parameter.span(),
            "a `#[queue]` marker takes no generic parameter: its identity is its type, so \
             every instantiation would share one wire name and one `#[process]` claim",
        )
        .to_compile_error()
        .into();
    }

    let ident = &item.ident;
    let QueueArgs { name, job } = args;

    quote! {
        #item

        impl ::nest_rs_queue::Queue for #ident {
            const NAME: &'static str = #name;
            type Job = #job;
        }

        impl ::nest_rs_queue::Destination for #ident {
            type Job = #job;

            fn queue_name(
                &self,
            ) -> ::std::result::Result<::nest_rs_queue::QueueName, ::nest_rs_queue::QueueError> {
                ::nest_rs_queue::QueueName::new(<Self as ::nest_rs_queue::Queue>::NAME)
            }
        }
    }
    .into()
}

/// The refusal of `#[queue]` on anything but a unit struct, naming what it was
/// written on.
fn not_a_unit_struct(shape: &str) -> String {
    format!(
        "#[queue] applies to a unit struct, and this is {shape} — a queue's identity is a type \
         carrying no data, the marker a push and a `#[process]` both name (e.g. \
         `#[queue(name = \"audio\", job = TranscodeCommand)] pub struct AudioQueue;`)"
    )
}

struct QueueArgs {
    name: LitStr,
    job: Type,
}

impl Parse for QueueArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name: Option<LitStr> = None;
        let mut job: Option<Type> = None;

        QUEUE.parse(input, |arg| {
            if arg.key() == "job" {
                job = Some(arg.value()?.parse::<Type>().map_err(|stopped| {
                    syn::Error::new(
                        stopped.span(),
                        takes_value(
                            "queue",
                            Some("job"),
                            "the type this queue's jobs carry, e.g. `job = TranscodeCommand`",
                        ),
                    )
                })?);
            } else {
                let literal = arg.str_lit("emails")?;
                // The rule `QueueName` states, refused at the literal.
                if !is_valid_queue_name(&literal.value()) {
                    return Err(syn::Error::new(
                        literal.span(),
                        invalid_queue_name("queue", arg.key(), &literal.value()),
                    ));
                }
                name = Some(literal);
            }
            Ok(())
        })?;

        let name = name.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                missing_argument("queue", "name", "\"emails\""),
            )
        })?;
        let job = job.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                format!(
                    "{} (the type this queue's jobs carry)",
                    missing_argument("queue", "job", "<PayloadType>"),
                ),
            )
        })?;

        Ok(Self { name, job })
    }
}
