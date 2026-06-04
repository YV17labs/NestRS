//! `#[on_event]` implementation: construction + `Discoverable` attaching an
//! `EventHandlerMeta` whose thunk subscribes the handler to the bus at bootstrap.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

use nestrs_codegen::{
    InjectableBody, build_injectable_body, from_container_method, injected_method,
};

pub(crate) fn on_event(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[on_event] takes no arguments; the handled event is the `type Event` \
             of the `EventHandler` impl",
        )
        .to_compile_error()
        .into();
    }

    let mut item = parse_macro_input!(input as ItemStruct);
    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let name_lit = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let injected = injected_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #injected

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                builder.attach_meta::<Self, ::nestrs_events::EventHandlerMeta>(
                    ::nestrs_events::EventHandlerMeta {
                        name: #name_lit,
                        // Built from the assembled container and subscribed at
                        // bootstrap, so the handler may inject any provider.
                        wire: |__container, __bus| {
                            let __handler = ::std::sync::Arc::new(
                                <Self>::from_container(__container),
                            );
                            __bus.subscribe::<
                                <Self as ::nestrs_events::EventHandler>::Event, _, _,
                            >(move |__event| {
                                let __handler = ::std::sync::Arc::clone(&__handler);
                                async move {
                                    ::nestrs_events::EventHandler::handle(&*__handler, __event).await
                                }
                            });
                        },
                    },
                )
            }
        }
    }
    .into()
}
