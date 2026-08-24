use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, ItemStruct, LitStr, Meta, Token};

use nest_rs_codegen::{
    DecoratorPair, Edge, InjectableBody, build_injectable_body, from_container_method,
    guard_capability_bounds, injected_keys_with_layers, injected_names_with_layers, layer_deps,
    reject_http_only_layers, require_str_lit, scoped_specs, take_path_list,
};

/// The MCP edge's pair. Naming the sibling is the whole point of the split: the
/// shape the developer reached for exists, it is just spelled with the other
/// decorator — and `parse_host` / `parse_operations` parse the item *before*
/// complaining, which is what keeps a broken `impl` from being reported as
/// "expected struct".
pub(crate) const MCP_PAIR: DecoratorPair = DecoratorPair {
    host: "#[mcp]",
    subject: "host struct",
    operations: "#[tools]",
    collects: "#[tool] / #[prompt]",
};

pub(crate) fn mcp(args: TokenStream, input: TokenStream) -> TokenStream {
    match MCP_PAIR.parse_host(input.into()) {
        Ok(item) => mcp_struct(args, item),
        Err(err) => err.to_compile_error().into(),
    }
}

/// `#[tools]` — the operations half, on the host's inherent impl. Named for
/// what it collects, the way `#[routes]` and `#[messages]` are; it carries the
/// `#[prompt]` methods too, since a prompt is an operation this same host
/// serves and rmcp routes both through one `ServerHandler`.
pub(crate) fn tools(args: TokenStream, input: TokenStream) -> TokenStream {
    match MCP_PAIR.parse_operations(input.into()) {
        Ok(item) => crate::mcp_impl::mcp_impl(args, item),
        Err(err) => err.to_compile_error().into(),
    }
}

fn mcp_struct(args: TokenStream, mut item: ItemStruct) -> TokenStream {
    let args = match parse_mcp_args(args.into()) {
        Ok(parsed) => parsed,
        Err(err) => return err.to_compile_error().into(),
    };

    // Interceptors and filters have no per-operation seam on this transport, so
    // binding one here would be a silent no-op — named compile error instead,
    // the same answer GraphQL and WS give.
    if let Err(err) = reject_http_only_layers(&item.attrs, "MCP", "host") {
        return err.to_compile_error().into();
    }
    // Host-scope (provider) guard declarations — same shape and same mental
    // model as `#[controller] struct` + `#[resolver] struct` + `#[gateway]
    // struct`. Stored here so the impl-form macro folds them into every
    // operation's chain at runtime through `__nestrs_mcp_host_guard_specs()`.
    let guards = match take_path_list(&mut item.attrs, "use_guards") {
        Ok(paths) => paths,
        Err(err) => return err.to_compile_error().into(),
    };
    // Host-scope guards fold into the same per-operation chain the operations'
    // own do, so they run `Guard::check_mcp` and owe the same capability.
    let capability_bounds =
        guard_capability_bounds(guards.iter(), quote!(::nest_rs_guards::McpGuard));

    let InjectableBody {
        ctor,
        dep_keys,
        dep_names,
        ..
    } = match build_injectable_body(&mut item) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error().into(),
    };

    let name = item.ident.clone();
    let host_name = name.to_string();
    let (impl_generics, ty_generics, where_clause) = item.generics.split_for_impl();
    let from_container = from_container_method(&ctor);
    // The struct's `#[inject]` keys + host-scope guards + whatever the decorated
    // impl declared per operation. The last part is why this reads through
    // `__nestrs_mcp_operation_layers()`: `#[mcp]` on the struct is what emits
    // `Discoverable` (a host may serve a hand-written `ServerHandler` and have no
    // decorated impl at all), so the per-operation guards have to arrive from the
    // other half rather than be folded in here.
    let layers = layer_deps(guards.iter());
    let injected_keys = injected_keys_with_layers(&dep_keys, &layers);
    let injected_names = injected_names_with_layers(&dep_names, &layers);
    let guard_specs = scoped_specs(&guards, quote!(dyn ::nest_rs_guards::Guard));

    // Empty stands for "the host declared none": the crate substitutes its
    // default endpoint. The decorator keeps no default of its own, so the path
    // a host lands on has one home and cannot drift between the two crates.
    let path = args.path.unwrap_or_else(|| LitStr::new("", name.span()));
    let (identity_name, identity_title) =
        (opt_str(args.name.as_ref()), opt_str(args.title.as_ref()));

    let residency = MCP_PAIR.host_residency(&name, &item.generics);

    quote! {
        #item

        #capability_bounds

        #residency

        impl #impl_generics #name #ty_generics #where_clause {
            #from_container

            /// Host-scope `#[use_guards(...)]`, read by the impl-form macro on
            /// the cache miss that composes an operation's chain. Empty when
            /// none declared.
            #[doc(hidden)]
            pub fn __nestrs_mcp_host_guard_specs()
                -> ::std::vec::Vec<::nest_rs_guards::dispatch::ScopedGuardSpec>
            {
                #guard_specs
            }
        }

        impl #impl_generics ::nest_rs_core::Discoverable for #name #ty_generics #where_clause {
            fn injected() -> ::std::vec::Vec<::core::any::TypeId> {
                // An inherent associated fn wins over a trait one, so this is
                // the decorated impl's own list when there is one, and the
                // fallback's empty pair when the host writes rmcp by hand.
                use ::nest_rs_mcp::DefaultOperationLayers as _;
                let mut __keys: ::std::vec::Vec<::core::any::TypeId> = #injected_keys;
                __keys.extend(<Self>::__nestrs_mcp_operation_layers().0);
                __keys
            }

            fn injected_names() -> ::std::vec::Vec<&'static str> {
                use ::nest_rs_mcp::DefaultOperationLayers as _;
                let mut __names: ::std::vec::Vec<&'static str> = #injected_names;
                __names.extend(<Self>::__nestrs_mcp_operation_layers().1);
                __names
            }

            fn register(
                builder: ::nest_rs_core::ContainerBuilder,
            ) -> ::nest_rs_core::ContainerBuilder {
                // Contribute to the endpoint at `path` — the *first* host on a
                // path attaches the mount, every host attaches itself. The
                // default path, the grouping, the merge, the guard/context/
                // config resolution, the identity overlay and the duplicate-tool
                // boot check all live in the crate, so the mount policy is
                // testable rather than macro-expanded.
                ::nest_rs_mcp::register_host::<Self>(
                    builder,
                    #path,
                    #host_name,
                    ::nest_rs_mcp::McpIdentity::declared(#identity_name, #identity_title),
                    |__c| -> ::std::sync::Arc<dyn ::nest_rs_mcp::McpHost> {
                        ::std::sync::Arc::new(<Self>::from_container(__c))
                    },
                    || {
                        // An inherent associated fn wins over a trait one, so
                        // this is the host's real router — whether the impl-level
                        // `#[mcp]` emitted it (as `pub(crate)`, so it is nameable
                        // from here) or the host wrote rmcp's `#[tool_router]`
                        // itself — and an empty stand-in when it has neither.
                        // The boot check that catches a duplicate tool name is
                        // only as good as this list.
                        use ::nest_rs_mcp::DefaultToolRouter as _;
                        <Self>::tool_router().list_all()
                    },
                )
            }
        }
    }
    .into()
}

/// Everything `#[mcp(..)]` accepts. Every argument is optional: a bare `#[mcp]`
/// serves the default endpoint and lets the app's identity speak for it.
///
/// `path` is a literal — it is a route, and the same shape `#[controller]`
/// takes. The identity arguments stay whole expressions so a host can name
/// itself from its own build environment (`name = env!("CARGO_PKG_NAME")`);
/// each is passed to `McpIdentity::declared`, whose `Option<&str>` parameters
/// are what reject anything else, spanned on the offending expression.
///
/// The pair is what a host can honestly say: *which endpoint stands apart*. The
/// server's own `version` is not on that list — a feature library knows neither
/// the binary's version nor, on a shared endpoint, the whole surface — so it is
/// declared once by the app, through `McpModule::for_root`. Nor is any other
/// field of the identity: [`SERVER_FIELDS`] is the rest of them, each refused by
/// name and pointed at that same seam, because a key that exists and is
/// somebody's deserves an answer rather than a list of spellings.
///
/// Unlike a controller's, this path is not a namespace the host owns — nothing
/// nests under it. It names the one endpoint the host joins, which is why
/// peers share it verbatim.
#[derive(Default)]
struct McpArgs {
    path: Option<LitStr>,
    name: Option<Expr>,
    title: Option<Expr>,
}

fn parse_mcp_args(args: TokenStream2) -> syn::Result<McpArgs> {
    // Before this decorator's own unknown-key arm, because `version` is not a
    // typo here: it is the word a developer carries over from `#[controller(
    // version = "1")]`, where it selects an address. MCP's answer to that — the
    // path is the address, the server's version is the app's one declaration —
    // is worded once, in `nest-rs-codegen`, for every edge that refuses it.
    Edge::Mcp.reject_version(&args)?;
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)?;
    let mut parsed = McpArgs::default();
    for meta in metas {
        // Accepting a repeat drops one of two declarations and source order
        // decides which — here that is the path a host joins, i.e. which peers
        // share its endpoint, and the identity a client is told it reached.
        let reject_duplicate = |taken: bool, meta: &Meta, key: &str| -> syn::Result<()> {
            nest_rs_codegen::reject_duplicate_argument(taken, meta, "mcp", key)
        };
        match meta {
            Meta::NameValue(nv) if nv.path.is_ident("path") => {
                reject_duplicate(parsed.path.is_some(), &Meta::NameValue(nv.clone()), "path")?;
                parsed.path = Some(require_str_lit(&nv.value, "mcp", "path", "/mcp")?)
            }
            Meta::NameValue(nv) if nv.path.is_ident("name") => {
                reject_duplicate(parsed.name.is_some(), &Meta::NameValue(nv.clone()), "name")?;
                parsed.name = Some(nv.value)
            }
            Meta::NameValue(nv) if nv.path.is_ident("title") => {
                reject_duplicate(
                    parsed.title.is_some(),
                    &Meta::NameValue(nv.clone()),
                    "title",
                )?;
                parsed.title = Some(nv.value)
            }
            // Two different answers, and telling them apart is the point. A key
            // naming a field of the server's identity is a key that *exists* —
            // it is declared by the app, and the sentence says where. Anything
            // else is nobody's, and gets the list of what remains.
            other => {
                return Err(match server_field(&other) {
                    Some(field) => syn::Error::new_spanned(&other, field.refusal()),
                    None => nest_rs_codegen::unmatched_meta("mcp", &other, &ACCEPTED_KEYS),
                });
            }
        }
    }
    if let Some(path) = &parsed.path {
        check_path(path)?;
    }
    Ok(parsed)
}

/// The keys that remain once every one this decorator refuses has named its own
/// owner — so a key reaching the shared unknown-argument sentence is one nothing
/// in the framework has a home for, which is the only case a list of spellings
/// actually helps.
const ACCEPTED_KEYS: [&str; 3] = ["path", "name", "title"];

/// Where a host's own prose goes, for the fields whose per-operation twin is
/// what a developer reaching for them usually meant. A server-level field
/// refused without it answers "not here" and not "there".
const TOOL_DESCRIPTION: &str =
    "What this host's tools *do* belongs to each #[tool(description = \"…\")]";

/// A field of the server's identity: real, settable, and the **app's** to set.
///
/// One row per `McpIdentity` builder beyond the `name`/`title` pair a host
/// declares for itself (see `nest-rs-mcp`'s `identity.rs`, which the unit test
/// below reads so this list cannot fall behind it). Each is a key a host may
/// plausibly write, and writing it is not a typo — the field exists, it is just
/// declared at the seam that can honestly state it, since on an endpoint several
/// features share no single host sees the whole. So each owes the *same shape of
/// answer* `version` and `instructions` already gave: a sentence naming the seam
/// that takes it. A bare "unknown key" here is the silence `CLAUDE.md` counts as
/// a defect — it sends the developer looking for a spelling that does not exist.
struct ServerField {
    /// The key as a host writes it, which is the identity field's own name.
    key: &'static str,
    /// The `McpIdentity` call that takes it, spelled into the remedy so the
    /// sentence carries a line the developer can paste.
    declares: &'static str,
    /// What they may have meant instead, when the field has a per-operation
    /// twin. Empty when it has none.
    instead: &'static str,
}

/// Every identity field the app owns, refused by name.
///
/// `version` is absent because it is refused one step earlier and by a wider
/// mechanism — `Edge::Mcp` words that answer once for every edge that has no
/// client-selectable version, and it says more than "the app declares it".
const SERVER_FIELDS: [ServerField; 4] = [
    ServerField {
        key: "description",
        declares: "description(\"…\")",
        instead: TOOL_DESCRIPTION,
    },
    ServerField {
        key: "website_url",
        declares: "website_url(\"…\")",
        instead: "",
    },
    ServerField {
        key: "icons",
        declares: "icons([…])",
        instead: "",
    },
    ServerField {
        key: "instructions",
        declares: "instructions(\"…\")",
        instead: TOOL_DESCRIPTION,
    },
];

/// The identity field a `#[mcp]` argument names, if it names one. Keyed off the
/// path alone, so a bare `icons` and an `icons = [..]` get the same answer — a
/// host that reached for the key learns where it lives either way.
fn server_field(meta: &Meta) -> Option<&'static ServerField> {
    SERVER_FIELDS
        .iter()
        .find(|field| meta.path().is_ident(field.key))
}

impl ServerField {
    /// The refusal as the developer reads it: whose the field is, and the one
    /// call that takes it. Worded once for every row — three spellings of one
    /// sentence is how the list fell behind the struct in the first place.
    fn refusal(&self) -> String {
        let Self {
            key,
            declares,
            instead,
        } = self;
        let mut message = format!(
            "#[mcp] takes no `{key}` — that describes the server, not one host, so it is \
             declared once: McpModule::for_root(McpOptions {{ server: \
             Some(McpIdentity::new(name, version).{declares}), ..Default::default() }})",
        );
        if !instead.is_empty() {
            message.push_str(". ");
            message.push_str(instead);
        }
        message
    }
}

/// A host's `path` is the whole URL path a client is configured with. Written
/// empty it says nothing at all, and the argument that says nothing is the
/// absent one — two spellings for one mount is what the framework does not
/// ship.
fn check_path(path: &LitStr) -> syn::Result<()> {
    // The shared grammar refuses the empty string too, in the same words as its
    // two siblings — this used to be the only one of three `path` keys that
    // checked anything, and it checked one of the four ways of getting it wrong.
    nest_rs_codegen::reject_path("mcp", path)
}

/// An optional identity argument as the `Option<&str>` tokens
/// `McpIdentity::declared` takes.
fn opt_str(expr: Option<&Expr>) -> TokenStream2 {
    match expr {
        Some(value) => quote! { ::core::option::Option::Some(#value) },
        None => quote! { ::core::option::Option::None },
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ACCEPTED_KEYS, SERVER_FIELDS};

    /// The identity a host declares for *itself* — the pair
    /// `McpIdentity::declared` takes, and the only builders the scan below may
    /// find with no refusal behind them.
    const HOST_DECLARED: [&str; 2] = ["name", "title"];

    /// A row added later cannot ship a sentence that names nothing: the key it
    /// refuses, the call that takes it, and the seam that call belongs to all
    /// have to reach the message the developer reads.
    #[test]
    fn every_refused_field_names_its_key_and_the_seam_that_takes_it() {
        for field in &SERVER_FIELDS {
            let message = field.refusal();
            assert!(message.contains(field.key), "{message}");
            assert!(
                message.contains(field.declares),
                "{} names no call to paste: {message}",
                field.key,
            );
            assert!(
                message.contains("McpModule::for_root"),
                "{} names no seam: {message}",
                field.key,
            );
            assert!(
                !ACCEPTED_KEYS.contains(&field.key),
                "`{}` is refused by name, so the accepted-key list must not offer it",
                field.key,
            );
        }
    }

    /// The drift this file exists to close, executed rather than stated.
    ///
    /// `McpIdentity` grew `description`, `website_url` and `icons`; the
    /// decorator's answers did not, so a host reaching for one got a bare
    /// unknown key. A `*-macros` crate may not depend on its surface crate, so
    /// the check reads the source — the same shape, and the same reason, as
    /// `nest-rs-macro-hygiene`'s emissions scan.
    #[test]
    fn no_identity_field_reaches_a_host_without_an_answer() {
        let identity = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crate sits in crates/")
            .join("nest-rs-mcp/src/identity.rs");
        let source = std::fs::read_to_string(&identity)
            .unwrap_or_else(|err| panic!("{} is readable: {err}", identity.display()));

        // A field an app sets is a builder taking `self` by value: everything
        // `McpIdentity::new` does not already take.
        let builders: Vec<&str> = source
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("pub fn "))
            .filter_map(|rest| rest.split_once("(mut self,"))
            .map(|(name, _)| name)
            .collect();
        assert!(
            builders.len() >= 5,
            "the scan found {builders:?} — it is reading the wrong file",
        );

        for builder in builders {
            assert!(
                HOST_DECLARED.contains(&builder)
                    || SERVER_FIELDS.iter().any(|field| field.key == builder),
                "McpIdentity::{builder} is a field an app sets, so a host will reach for \
                 `#[mcp({builder} = …)]` and get a bare unknown key — give it a row in \
                 SERVER_FIELDS naming the seam that takes it",
            );
        }
    }
}
