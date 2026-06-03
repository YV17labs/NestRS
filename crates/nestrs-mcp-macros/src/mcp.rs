use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct};

use nestrs_codegen::{
    build_injectable_body, from_container_method, injected_method, parse_named_str_arg,
    InjectableBody,
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

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            #injected

            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                builder.attach_meta::<#name, ::nestrs_http::HttpEndpointMeta>(
                    ::nestrs_http::HttpEndpointMeta::new(#path, "mcp", |__c, __r| {
                        let __cc = __c.clone();
                        let __guard = __c.get_dyn::<dyn ::nestrs_mcp::McpOperationGuard>();
                        __r.nest(
                            #path,
                            ::nestrs_mcp::endpoint_with_guard(
                                __guard,
                                move || <#name>::from_container(&__cc),
                            ),
                        )
                    }),
                )
            }
        }
    }
    .into()
}
