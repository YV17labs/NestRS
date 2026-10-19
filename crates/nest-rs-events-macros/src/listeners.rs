//! `#[listeners]`: per `#[on_event]` method, a free `wire` fn and a
//! `ListenerMethod` inventory entry.

use nest_rs_codegen::__private::layers::{LayerScope, take_layers};
use nest_rs_codegen::__private::pair;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, quote_spanned};
use syn::ImplItem;
use syn::ext::IdentExt;

use nest_rs_codegen::__private::{
    HandlerSite, HostMeta, ProviderSite, SitePosture, VersionedEdge, answer_span, await_if_async,
    cfg_attrs, concrete_signature, handler_const, host_layers_entry, impl_self_ident,
    one_role_per_method, parse_on_event_args, payload_arg_type, site_const, site_ref,
};
use nest_rs_codegen::snake_case;

pub(crate) fn listeners(args: TokenStream, input: TokenStream) -> TokenStream {
    let written = TokenStream2::from(input.clone());
    let expansion = expand(args, input).into();
    pair::LISTENERS
        .keep_item_on_refusal(written, expansion, &["on_event"], |_| TokenStream2::new())
        .into()
}

fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    // `version` before the blanket refusal, which would answer another question.
    if let Err(err) = VersionedEdge::Events.reject_version(&args) {
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

    let host_layers = match take_layers(&pair::LISTENERS, LayerScope::Host, &mut item.attrs) {
        Ok(layers) => layers,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut emissions: Vec<TokenStream2> = Vec::new();
    let mut handler_consts: Vec<TokenStream2> = Vec::new();
    // `inventory` yields link order; this index restores the written one.
    let mut declaration_index: usize = 0;

    for impl_item in item.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let index = match one_role_per_method("listener", &method.attrs, &["on_event"], "") {
            Ok(Some(index)) => index,
            Ok(None) => continue,
            Err(err) => return err.to_compile_error().into(),
        };
        let attr = method.attrs.remove(index);
        if let Err(err) = concrete_signature(method, "#[on_event]") {
            return err.to_compile_error().into();
        }

        let args = match parse_on_event_args(&attr) {
            Ok(args) => args,
            Err(err) => return err.to_compile_error().into(),
        };
        let policy = args.transaction(&quote!(::nest_rs_events::__private));
        let timeout = args.timeout();

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
        let method_layers =
            match take_layers(&pair::LISTENERS, LayerScope::Method, &mut method.attrs) {
                Ok(layers) => layers,
                Err(err) => return err.to_compile_error().into(),
            };
        handler_consts.push(handler_const(&HandlerSite {
            unit: quote!(::nest_rs_events::unit::DISPATCH),
            self_ty: &self_ty,
            host_name: &provider_ident,
            method: &method_ident,
            address: quote!(::core::stringify!(#event_ty)),
            posture: SitePosture::System,
            host_meta: HostMeta::Values(&host_layers.meta),
            method_meta: &method_layers.meta,
            cfgs: &cfgs,
        }));
        let site = ProviderSite {
            self_ty: &self_ty,
            method: &method_ident,
            host: &host_layers,
            method_layers: &method_layers,
            cfgs: &cfgs,
        };
        handler_consts.push(site_const(&site));
        emissions.push(host_layers_entry(&site));
        let site_ref = site_ref(&self_ty, &method_ident);
        let call = await_if_async(
            &method.sig,
            quote!(<#self_ty>::#method_ident(&__provider, __event)),
        );
        // What the method answers is read by its type, refused at its return type.
        let answered = quote_spanned! {answer_span(&method.sig)=>
            ::nest_rs_events::__private::answered(__nestrs_answer)
        };

        emissions.push(quote! {
            #(#cfgs)*
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #wire_ident(
                __container: &::nest_rs_core::Container,
                __bus: &::nest_rs_events::EventBus,
            ) -> ::nest_rs_core::anyhow::Result<()> {
                let __provider = ::nest_rs_core::Container::get::<#self_ty>(__container)
                    .expect(::std::concat!(
                        "listeners provider `",
                        #provider_name,
                        "` is not registered — add it to a reachable module's \
                         `providers = [...]`",
                    ));
                ::nest_rs_events::__private::subscribe_named::<#event_ty, _, _>(
                    __bus,
                    __container,
                    #site_ref,
                    #qualified_name,
                    #policy,
                    #timeout,
                    move |__event| {
                        let __provider = ::std::sync::Arc::clone(&__provider);
                        async move {
                            let __nestrs_answer = #call;
                            #answered
                        }
                    },
                )
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

        impl #self_ty {
            #(#handler_consts)*
        }

        #host_check
        #(#emissions)*
    };
    out.into()
}
