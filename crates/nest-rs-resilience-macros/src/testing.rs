//! What the unit tests of every file share: an item expanded or refused, and
//! the methods they decorate.

use proc_macro2::TokenStream;
use quote::quote;

use crate::resilient::{Refused, decorated};

pub(crate) fn refusals(args: TokenStream, item: TokenStream) -> Vec<String> {
    match decorated(args, item) {
        Ok(expanded) => panic!("expanded, refusing nothing: {expanded}"),
        Err(Refused::Method(_, refusal) | Refused::Item(_, refusal)) => refusal
            .into_iter()
            .map(|refusal| refusal.to_string())
            .collect(),
    }
}

/// What the attribute emits for a refused `item`.
pub(crate) fn emitted(args: TokenStream, item: TokenStream) -> String {
    match decorated(args, item) {
        Ok(expanded) => panic!("expanded, refusing nothing: {expanded}"),
        Err(refused) => refused.emitted().to_string(),
    }
}

pub(crate) fn track() -> TokenStream {
    quote!(
        async fn track(&self) -> Answer {
            send().await
        }
    )
}

/// `ship` as tracing-attributes 0.1.31 leaves it when its instrument is
/// written above the decorator, whose body is `written`.
pub(crate) fn instrumented_above(written: TokenStream) -> TokenStream {
    quote! {
        async fn ship(&self, order: Order) -> Answer {
            {}
            let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
            let __tracing_instrument_future = async move {
                #[allow(unreachable_code)]
                if false {
                    let __tracing_attr_fake_return: Answer = loop {};
                    return __tracing_attr_fake_return;
                }
                { #written }
            };
            if !__tracing_attr_span.is_disabled() {
                ::tracing::Instrument::instrument(
                    __tracing_instrument_future,
                    __tracing_attr_span
                )
                .await
            } else {
                __tracing_instrument_future.await
            }
        }
    }
}
