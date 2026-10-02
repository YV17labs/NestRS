//! `#[listeners]` — orchestrator on a provider's `impl` block. Walks the
//! methods; for each one tagged with `#[on_event]`, emits a free `wire` fn
//! that resolves the provider from the assembled container and subscribes a
//! closure to the [`EventBus`], then submits a `ListenerMethod` inventory
//! entry the [`EventsModule`] drains at bootstrap.
//!
//! Mirrors `#[processor]`/`#[process]` and `#[scheduled]`/`#[every]`: the
//! host struct keeps its own `#[injectable]` (which owns `Discoverable`), and
//! several decorated methods pool the provider's `#[inject]` dependencies.
//!
//! `#[on_event]` is a pure marker consumed here — it is not registered as a
//! proc-macro attribute, so writing it outside a `#[listeners]` impl block
//! fails the same way `#[get]` outside `#[routes]` does.

use nest_rs_codegen::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::ImplItem;
use syn::ext::IdentExt;

use nest_rs_codegen::{
    Edge, await_if_async, cfg_attrs, impl_self_ident, payload_arg_type, returns_unit, snake_case,
};

pub(crate) fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::LISTENERS
        .keep_item_on_refusal(written, expansion, &["on_event"], |_| TokenStream2::new())
        .into()
}

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    // `version` before the blanket refusal: the developer arriving from
    // `#[controller(version = "1")]` asked a real question, and "takes no
    // arguments" answers a different one. The sentence is `nest-rs-codegen`'s,
    // so this edge's answer is worded where every edge's is.
    if let Err(err) = Edge::Events.reject_version(&args) {
        return err.to_compile_error().into();
    }
    if let Err(err) = pair::LISTENERS.reject_args(&args, "the provider's scope is declared by") {
        return err.to_compile_error().into();
    }

    let mut item = match pair::LISTENERS.parse_operations(input.into()) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error().into(),
    };
    let self_ty = item.self_ty.clone();
    let host_check = pair::LISTENERS.provider_host_check(&self_ty);
    let provider_ident = match impl_self_ident(&self_ty, "#[listeners]") {
        Ok(ident) => ident,
        Err(err) => return err.to_compile_error().into(),
    };
    let provider_name = provider_ident.unraw().to_string();
    let provider_snake = snake_case(&provider_name);

    let mut emissions: Vec<TokenStream2> = Vec::new();
    // Position of each `#[on_event]` method **in this block**, submitted with
    // the entry. `inventory` hands entries back in link order, which is stable
    // per binary and reshuffles whenever the code changes — so two listeners
    // ordered deliberately and verified locally were silently rearranged the
    // next time somebody added a third. The index is what lets `EventsModule`
    // restore the order the developer actually wrote.
    let mut declaration_index: usize = 0;

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let index = match nest_rs_codegen::one_role_per_method(
            "listener",
            &method.attrs,
            &["on_event"],
            "",
        ) {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(err) => return err.to_compile_error().into(),
        };
        let attr = method.attrs.remove(index);
        if let Err(err) = nest_rs_codegen::concrete_signature(method, "#[on_event]") {
            return err.to_compile_error().into();
        }

        if attr.meta.require_path_only().is_err() {
            return syn::Error::new_spanned(
                attr,
                "#[on_event] takes no arguments; the event is read from the method's \
                 second parameter (e.g. `fn on_x(&self, event: PointsAwarded)`)",
            )
            .to_compile_error()
            .into();
        }

        if !returns_unit(&method.sig.output) {
            return syn::Error::new_spanned(
                &method.sig.output,
                "#[on_event] methods are fire-and-forget — return `()`, written out or left \
                 out (an alias of `()` is not read as one), and handle errors inside the \
                 method (push a failed job to the queue, log, etc.)",
            )
            .to_compile_error()
            .into();
        }

        let event_ty = match payload_arg_type(method, "#[on_event]", "event", &provider_ident) {
            Ok(ty) => ty,
            Err(err) => return err.to_compile_error().into(),
        };

        let method_ident = method.sig.ident.clone();
        // Un-raw: a label is read, and a generated identifier cannot hold `r#`.
        let method_name = method_ident.unraw().to_string();
        let qualified_name = format!("{provider_name}::{method_name}");
        let method_snake = snake_case(&method_name);
        let wire_ident =
            format_ident!("__nestrs_listener_wire_{}_{}", provider_snake, method_snake);

        let declaration = proc_macro2::Literal::usize_unsuffixed(declaration_index);
        declaration_index += 1;
        let cfgs = cfg_attrs(&method.attrs);
        let call = await_if_async(
            &method.sig,
            quote!(<#self_ty>::#method_ident(&__provider, __event)),
        );

        emissions.push(quote! {
            #(#cfgs)*
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #wire_ident(
                __container: &::nest_rs_core::Container,
                __bus: &::nest_rs_events::EventBus,
            ) {
                let __provider = ::nest_rs_core::Container::get::<#self_ty>(__container)
                    .expect(::std::concat!(
                        "listeners provider `",
                        #provider_name,
                        "` is not registered — add it to a reachable module's \
                         `providers = [...]`",
                    ));
                __bus.subscribe_named::<#event_ty, _, _>(#qualified_name, move |__event| {
                    let __provider = ::std::sync::Arc::clone(&__provider);
                    async move {
                        #call
                    }
                });
            }

            #(#cfgs)*
            ::nest_rs_core::inventory::submit! {
                ::nest_rs_events::ListenerMethod {
                    origin: ::core::module_path!(),
                    name: #qualified_name,
                    provider_type_id: || ::std::any::TypeId::of::<#self_ty>(),
                    declaration_index: #declaration,
                    wire: #wire_ident,
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
