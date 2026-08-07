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
    let host_name = name.to_string();
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
                // Contribute to the endpoint at `path` — the *first* host on a
                // path attaches the mount, every host attaches itself. Grouping,
                // the merge, the guard/context/config resolution and the
                // duplicate-tool boot check all live in the crate, so the mount
                // policy is testable rather than macro-expanded.
                ::nest_rs_mcp::register_host::<Self>(
                    builder,
                    #path,
                    #host_name,
                    |__c| -> ::std::sync::Arc<dyn ::nest_rs_mcp::McpHost> {
                        ::std::sync::Arc::new(<Self>::from_container(__c))
                    },
                    || {
                        // An inherent associated fn wins over a trait one, so
                        // this is rmcp's `#[tool_router]`-generated router when
                        // the host has one, and an empty stand-in when it does
                        // not — no second decorator, no manifest line.
                        use ::nest_rs_mcp::DefaultToolRouter as _;
                        <Self>::tool_router().list_all()
                    },
                )
            }
        }
    }
    .into()
}
