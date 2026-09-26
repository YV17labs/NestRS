//! `#[queue(name = "…", job = Payload)]` — the attribute that gives a unit
//! struct a queue's compile-time identity: its wire name and its payload type.
//! Emits absolute `::nest_rs_queue::*` paths so the macros crate never depends
//! on its surface crate.

use nest_rs_codegen::{
    invalid_queue_name, is_valid_queue_name, missing_argument, needs_a_value,
    reject_duplicate_argument, require_str_lit, takes_value, unknown_argument,
};
use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Expr, Ident, Item, LitStr, Token, Type, parse_macro_input};

/// Every key `#[queue]` takes, in the order its unknown-key refusal lists them.
const KEYS: [&str; 2] = ["name", "job"];

/// The key 6.x offered for a queue per runtime key, refused by name rather than
/// as unknown: a developer writing it is owed the reason it is gone and what to
/// write instead.
const PREFIX: &str = "prefix";

/// Why `#[queue]` takes no `prefix`, in the facts a reader can check.
const NO_PREFIX: &str = "#[queue] takes no `prefix`: one queue per runtime key is not offered — the \
     Redis backend drains every queue from a list of its own, polled by each worker replica, so a \
     queue per key would cost Redis a poller per key and leave an autoscaler no single list to \
     read. Declare one queue with `name` and carry the key in the job";

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
            // Name the shape the developer actually wrote, as `#[input]` does,
            // rather than syn's `expected struct`.
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

    // A unit struct cannot carry a type or lifetime parameter (`E0392`), but it
    // *can* carry a const one — and `Q<1>` and `Q<2>` would then share one wire
    // name, so two markers claim one queue with nothing saying so. The identity
    // is the type, so the type is concrete.
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

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            // The key is judged before the `=`, so a bare `#[queue(name)]` names
            // the key rather than dying on syn's `` expected `=` `` — and a bare
            // *unknown* key still reads as unknown rather than as missing a value.
            let spelled = key.to_string();
            if spelled == PREFIX {
                return Err(syn::Error::new(key.span(), NO_PREFIX));
            }
            if !KEYS.contains(&spelled.as_str()) {
                return Err(syn::Error::new(
                    key.span(),
                    unknown_argument("queue", &spelled, &KEYS),
                ));
            }
            if !input.peek(Token![=]) {
                return Err(syn::Error::new(
                    key.span(),
                    needs_a_value("queue", &spelled),
                ));
            }
            input.parse::<Token![=]>()?;
            if spelled == "job" {
                reject_duplicate_argument(job.is_some(), &key, "queue", &spelled)?;
                job = Some(input.parse::<Type>().map_err(|stopped| {
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
                reject_duplicate_argument(name.is_some(), &key, "queue", &spelled)?;
                let literal = require_str_lit(&input.parse::<Expr>()?, "queue", "name", "emails")?;
                // The rule `QueueName` states, refused at the literal.
                if !is_valid_queue_name(&literal.value()) {
                    return Err(syn::Error::new(
                        literal.span(),
                        invalid_queue_name("queue", &spelled, &literal.value()),
                    ));
                }
                name = Some(literal);
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

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
