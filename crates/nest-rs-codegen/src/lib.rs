//! Shared token helpers for nestrs decorators.
//!
//! The crate's root items are a kit for a third-party `proc-macro` crate, and
//! semver covers them: [`Grammar`] and [`Arg`] read a decorator's keys and word
//! its refusals as every framework decorator does, [`Refusals`] reports every
//! refusal of an item at once, [`duration_millis`] reads a duration,
//! [`pascal_case`] and [`snake_case`] derive a name, [`take_flag_attr`],
//! [`take_single_attr`] and [`take_path_list`] take a marker attribute off an
//! item ([`repeated_attribute`] words a second copy), and [`reroot`] roots what
//! the expansion emits. Everything else in the crate is the framework's own and
//! no API.
//!
//! A decorator of your own keeps two rules:
//!
//! 1. **It emits a framework path as `::nest_rs_<concern>::<item>`**, an item
//!    the concern's documentation shows, and passes its finished expansion
//!    through [`reroot`], which rewrites each such root to the path the call
//!    site resolves — `::nest_rs::<concern>::` behind the umbrella. It never
//!    emits a hidden path: a package's macro source is written against one
//!    7.x and runs in apps on later ones, where hidden items differ, and only
//!    documented items hold across them.
//! 2. **It emits its own package's items as `::<its_crate>::…`.**
//!
//! Depend on `nest-rs-codegen = "7.N"` from a `proc-macro` crate only; the
//! crate's root names `syn` 3 and `proc-macro2` 1 types, so their majors move
//! with nestrs's.
//!
//! ```
//! use nest_rs_codegen::{Grammar, reroot};
//! use proc_macro2::TokenStream;
//! use quote::quote;
//!
//! const AUDITED: Grammar = Grammar::new("audited", &["topic"]);
//!
//! fn audited(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
//!     let mut topic = None;
//!     AUDITED.parse2(args, |arg| {
//!         topic = Some(arg.str_lit("billing")?.value());
//!         Ok(())
//!     })?;
//!     let item: syn::ItemStruct = syn::parse2(item)?;
//!     let ident = &item.ident;
//!     let topic = topic.unwrap_or_else(|| nest_rs_codegen::snake_case(&ident.to_string()));
//!     Ok(reroot(quote! {
//!         #item
//!         impl ::nestrs_audit::Audited for #ident {
//!             const TOPIC: &'static str = #topic;
//!         }
//!     }))
//! }
//! # let expanded = audited(quote!(), quote!(struct OrderLine;))?;
//! # assert!(expanded.to_string().contains("\"order_line\""));
//! # let refused = audited(quote!(topc = "billing"), quote!(struct Invoice;)).unwrap_err();
//! # assert_eq!(refused.to_string(), "unknown #[audited] argument `topc`; expected `topic`");
//! # Ok::<(), syn::Error>(())
//! ```
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod args;
mod attrs;
mod capability;
mod casing;
mod crud;
mod deprecation;
mod dispatch;
mod duration;
mod entry;
mod grammar;
mod identity;
mod inject;
mod instance;
mod job;
mod layers;
mod mcp;
mod meta;
mod mount;
mod on_event;
mod pair;
mod posture;
mod provider_site;
mod queue_name;
mod refusals;
mod replicas;
mod root;
mod route_path;
mod specs;
mod time_zone;
mod ty;
mod ungrouped;
mod versioning;

pub use attrs::{repeated_attribute, take_flag_attr, take_path_list, take_single_attr};
pub use casing::{pascal_case, snake_case};
pub use duration::duration_millis;
pub use grammar::{Arg, Grammar};
pub use refusals::Refusals;
pub use root::reroot;

#[doc(hidden)]
pub mod __private {
    //! Called by the framework's own `*-macros` crates. Not API: may change in
    //! any release, so a third-party macro crate names none of it.
    //!
    //! ```compile_fail
    //! let member = nest_rs_codegen::JobDecorator::Every;
    //! ```

    pub use crate::args::{
        duplicate_argument, key_as_written, missing_argument, needs_a_value, one_role_per_method,
        require_str_lit, site, takes_value, unknown_argument, unknown_value,
    };
    pub use crate::attrs::{Conditional, cfg_attrs, delegated_attrs};
    pub use crate::capability::guard_capability_bounds;
    pub use crate::crud::{
        CrudDeclaration, CrudOp, GeneratedOps, OpsSelection, Paginate, parse_crud_args, singular_of,
    };
    pub use crate::deprecation::{deprecation_header, refuse_rust_deprecated};
    pub use crate::dispatch::{Collision, DispatchKeys};
    pub use crate::entry::{ENTRY, entry_needs_an_async_fn, entry_takes_no_arguments};
    pub use crate::identity::{
        KEY, invalid_job_key, is_valid_job_key, key_value, key_without_replicas_one,
    };
    pub use crate::inject::{
        InjectableBody, LayerDeps, build_injectable_body, dependencies_method,
        dependency_names_method, forwarded_arg_idents, forwarded_idents, from_container_method,
        from_scope_method, injected_keys_with_layers, injected_method,
        injected_methods_with_layers, injected_names_method, injected_names_with_layers,
        injected_optional_fns, injected_optional_method, injected_optional_names_method,
        injected_per_request_method, layer_deps, mixed_site_ident, normalize_forwarded_args,
        optional_dependencies_method, optional_dependency_names_method,
    };
    pub use crate::instance::{DEFAULT_INSTANCE, INSTANCE, RESERVED_VOCABULARY, instance_name};
    pub use crate::job::{
        JobDecorator, JobKey, TIMEOUT, TRANSACTIONAL, job_argument_needs_a_value, job_key,
        job_keys, job_returns_a_result, job_timeout, job_transaction, timeout_value,
        transactional_value, unread_job_key,
    };
    pub use crate::mcp::{MCP_GRAMMAR, mcp_answers};
    pub use crate::meta::{
        HandlerDeclaration, HandlerSite, HostMeta, SitePosture, declared_handler, handler_const,
        handler_ref, host_meta_fn, reject_meta_on_operations, take_meta,
    };
    pub use crate::mount::{MountPath, reject_path};
    pub use crate::on_event::{ON_EVENT_GRAMMAR, OnEventArgs, parse_on_event_args};
    pub use crate::pair::{DecoratorPair, parse_provider_host, provider_residency};
    pub use crate::posture::{
        ID_ARG_UNSUPPORTED_BECAUSE, Posture, PostureRules, at_most_one_authorize,
        posture_contradiction, posture_key_unsupported, posture_required,
    };
    pub use crate::provider_site::{ProviderSite, host_layers_entry, site_const, site_ref};
    pub use crate::queue_name::{invalid_queue_name, is_valid_queue_name};
    pub use crate::replicas::{REPLICAS, Replicas, replicas_value};
    pub use crate::route_path::RoutePath;
    pub use crate::specs::{force_guard_typeids, scoped_specs};
    pub use crate::time_zone::invalid_time_zone;
    pub use crate::ty::{
        HostBorrow, PipeWrapper, answer_span, await_if_async, concrete_signature, generic_args,
        impl_self_ident, last_segment_ident, nth_generic_type, path_label, payload_arg_type,
        pipe_wrapper, returns_unit, shared_receiver, type_label,
    };
    pub use crate::ungrouped::ungrouped_expr;
    pub use crate::versioning::{VersionAnswer, VersionedEdge};

    /// Every decorator pair: the closed table both halves parse through.
    pub mod pair {
        pub use crate::pair::{
            ALL, GRAPHQL, HOOKS, HTTP, INDICATORS, LISTENERS, MCP, PROCESSOR, PairKind, SCHEDULED,
            WS,
        };
    }

    /// The layer grammar: which layer attribute every pair takes, and where.
    pub mod layers {
        pub use crate::layers::{
            Cell, LayerAttribute, LayerScope, Layers, cell, refusal, take_layers,
        };
    }

    /// The version grammar, which `nest-rs-http` pins its runtime copy against.
    pub mod versioning {
        pub use crate::versioning::{
            MAX_VERSION_LEN, is_valid_version, parse_version_args, parse_version_list,
        };
    }
}
