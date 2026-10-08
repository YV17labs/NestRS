//! Shared token-building helpers for nestrs decorator macros.
//!
//! Proc-macro crates can only export macros, so the logic every decorator
//! shares lives here; add new helpers here rather than in a `*-macros` crate.
//!
//! This crate never depends on a surface crate: emitted absolute paths
//! (`::nest_rs_core::*`) resolve at the call site, through [`reroot`].
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod args;
mod attrs;
mod capability;
mod casing;
mod crud;
mod dispatch;
mod duration;
mod entry;
mod grammar;
mod identity;
mod inject;
mod job;
mod mcp;
mod mount;
pub mod pair;
mod posture;
mod queue_name;
mod replicas;
mod root;
mod route_path;
mod specs;
mod time_zone;
mod ty;
mod ungrouped;
/// Public because `nest-rs-http` cannot depend on this crate (it pulls `syn`):
/// its runtime copy of the grammar is pinned against this one by path in a test.
pub mod versioning;

pub use args::{
    duplicate_argument, key_as_written, missing_argument, needs_a_value, one_role_per_method,
    require_str_lit, site, takes_value, unknown_argument, unknown_value,
};
pub use attrs::{
    Conditional, cfg_attrs, delegated_attrs, reject_http_only_layers, repeated_attribute,
    take_flag_attr, take_path_list, take_single_attr,
};
pub use capability::guard_capability_bounds;
pub use casing::{pascal_case, snake_case};
pub use crud::{
    CrudDeclaration, CrudOp, GeneratedOps, OpsSelection, Paginate, parse_crud_args, singular_of,
};
pub use dispatch::{Collision, DispatchKeys};
pub use duration::duration_millis;
pub use entry::{ENTRY, entry_needs_an_async_fn, entry_takes_no_arguments};
pub use grammar::{Arg, Grammar};
pub use identity::{KEY, invalid_job_key, is_valid_job_key, key_value, key_without_replicas_one};
pub use inject::{
    InjectableBody, LayerDeps, build_injectable_body, dependencies_method, dependency_names_method,
    forwarded_arg_idents, forwarded_idents, from_container_method, from_scope_method,
    injected_keyed_method, injected_keys_with_layers, injected_method,
    injected_methods_with_layers, injected_names_method, injected_names_with_layers,
    injected_optional_method, layer_deps, mixed_site_ident, normalize_forwarded_args,
    optional_dependencies_method,
};
pub use job::{
    JobDecorator, JobKey, TIMEOUT, TRANSACTIONAL, job_argument_needs_a_value, job_key, job_keys,
    job_returns_a_result, job_timeout, job_transaction, timeout_value, transactional_value,
    unread_job_key,
};
pub use mcp::{MCP_GRAMMAR, mcp_answers};
pub use mount::reject_path;
pub use pair::{DecoratorPair, parse_provider_host, provider_residency};
pub use posture::{
    ID_ARG_UNSUPPORTED_BECAUSE, Posture, PostureRules, at_most_one_authorize,
    posture_contradiction, posture_key_unsupported, posture_required,
};
pub use queue_name::{invalid_queue_name, is_valid_queue_name};
pub use replicas::{REPLICAS, Replicas, replicas_value};
pub use root::reroot;
pub use route_path::RoutePath;
pub use specs::{force_guard_typeids, scoped_specs};
pub use time_zone::invalid_time_zone;
pub use ty::{
    HostBorrow, PipeWrapper, await_if_async, concrete_signature, generic_args, impl_self_ident,
    last_segment_ident, nth_generic_type, payload_arg_type, pipe_wrapper, returns_unit,
    shared_receiver, type_label,
};
pub use ungrouped::ungrouped_expr;
pub use versioning::{Edge, VersionAnswer};
