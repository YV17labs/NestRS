use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemStruct, parse_macro_input};

use nest_rs_codegen::{
    InjectableBody, build_injectable_body, from_container_method, injected_method,
    parse_named_str_arg,
};

pub(crate) fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    let path = match parse_named_str_arg(args.into(), "path", "mcp") {
        Ok(path) => path,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody { ctor, dep_keys, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    let injected = injected_method(&dep_keys);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            #injected

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                builder.attach_meta::<#name, ::nest_rs_http::HttpEndpointMeta>(
                    ::nest_rs_http::HttpEndpointMeta::new(#path, "mcp", |__c, __r| {
                        let __cc = __c.clone();
                        // Guard (registered bridge → global guard pool →
                        // deny-all), data context, `McpConfig` and any session
                        // store resolve in one place — the crate — so the mount
                        // policy is testable rather than macro-expanded.
                        let __mount = ::nest_rs_mcp::McpMount::from_container(__c);
                        __r.nest(
                            #path,
                            ::nest_rs_mcp::endpoint(
                                __mount,
                                move || <#name>::from_container(&__cc),
                            ),
                        )
                    }).exempt(),
                )
            }
        }
    }
    .into()
}
