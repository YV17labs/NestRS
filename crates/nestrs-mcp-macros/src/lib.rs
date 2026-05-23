//! MCP decorator macro, re-exported by `nestrs-mcp`. The generated code uses
//! absolute paths (`::nestrs_mcp::*`, `::nestrs_http::*`, `::nestrs_core::*`,
//! `::poem::*`), so this crate does not depend on them — they resolve at the
//! call site.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct};

use nestrs_macro_support::{
    build_injectable_body, from_container_method, parse_named_str_arg, InjectableBody,
};

// -----------------------------------------------------------------------------
// #[mcp(path = "...")]
// -----------------------------------------------------------------------------

/// Mark a struct as an MCP server handler that mounts itself over HTTP.
///
/// Behaves like `#[injectable]` for construction (fields with `#[inject]`
/// resolved from the container, others default) and additionally emits an
/// `impl Discoverable` that attaches an `HttpEndpointMeta`. Listed in a
/// `#[module]`, the handler serves an MCP streamable-HTTP endpoint at `path`
/// with no `.mount()` call in `main.rs`.
///
/// The struct must carry the `rmcp` `#[tool_router]` / `#[tool_handler]`
/// impls — `nestrs_mcp::endpoint` requires `ServerHandler`. The factory runs
/// per session, so the handler is rebuilt from the container each time and
/// any per-session state stays fresh.
#[proc_macro_attribute]
pub fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    let path = match parse_named_str_arg(args.into(), "path", "mcp") {
        Ok(path) => path,
        Err(err) => return err.to_compile_error().into(),
    };
    let mut item = parse_macro_input!(input as ItemStruct);

    let InjectableBody { ctor, .. } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);

    quote! {
        #item

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container
        }

        impl #impl_generics ::nestrs_core::Discoverable for #name #ty_generics #where_clause {
            fn register(
                builder: ::nestrs_core::ContainerBuilder,
            ) -> ::nestrs_core::ContainerBuilder {
                builder.attach_meta::<#name, ::nestrs_http::HttpEndpointMeta>(
                    ::nestrs_http::HttpEndpointMeta::new(#path, "mcp", |__c, __r| {
                        let __cc = __c.clone();
                        __r.nest(
                            #path,
                            ::nestrs_mcp::endpoint(move || <#name>::from_container(&__cc)),
                        )
                    }),
                )
            }
        }
    }
    .into()
}
