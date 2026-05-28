//! The `#[event_handler]` decorator, re-exported by `nestrs-events`. The generated
//! code uses absolute paths (`::nestrs_events::*`, `::nestrs_core::*`, `::std::*`),
//! so this crate does not depend on them — they resolve at the call site.
//! Token-building helpers are shared with the other decorators via `nestrs-codegen`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, ItemStruct};

use nestrs_codegen::{
    build_injectable_body, from_container_method, injected_method, InjectableBody,
};

/// Mark a struct as an event handler, discovered like a controller or cron job.
///
/// Construction mirrors `#[injectable]` — fields tagged `#[inject]` are resolved
/// from the container, others default, and the macro emits `from_container`. It
/// additionally emits `impl Discoverable` attaching an `EventHandlerMeta` whose
/// thunk builds the handler from the (fully-assembled) container and subscribes it
/// to the [`EventBus`](../nestrs_events/struct.EventBus.html). The struct must
/// implement [`EventHandler`](../nestrs_events/trait.EventHandler.html), which
/// declares the `Event` type it handles.
///
/// ```ignore
/// #[event_handler]
/// pub struct SendWelcomeEmail {
///     #[inject] mailer: std::sync::Arc<Mailer>,
/// }
///
/// #[nestrs_events::async_trait]
/// impl nestrs_events::EventHandler for SendWelcomeEmail {
///     type Event = UserRegistered;
///     async fn handle(&self, event: UserRegistered) {
///         self.mailer.welcome(event.email).await;
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn event_handler(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    if !args.is_empty() {
        return syn::Error::new_spanned(
            &args,
            "#[event_handler] takes no arguments; the handled event is the `type Event` \
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
