//! Shared token-building helpers for nestrs decorator macros.
//!
//! Proc-macro crates can only export macros, so the logic every decorator
//! shares lives here (a plain library crate) and each `nest-rs-*-macros` crate
//! depends on it. New decorators should reuse the helpers below — and add
//! new ones here rather than in a `*-macros` crate, so third-party decorators
//! can use them too.
//!
//! This crate never depends on `nest-rs-core` or any other surface crate:
//! emitted absolute-path tokens (`::nest_rs_core::*`) resolve at the call site.
//! Which root actually resolves there depends on what the call site declared —
//! see [`reroot`], which every decorator applies to what it returns.
#![warn(missing_docs)]

mod args;
mod attrs;
mod capability;
mod casing;
mod crud;
mod dispatch;
mod duration;
mod inject;
mod job;
mod mount;
mod pair;
mod posture;
mod queue_name;
mod replicas;
mod root;
mod route_path;
mod specs;
mod ty;
mod ungrouped;
/// Public because its runtime counterpart needs a *name* from it: `nest-rs-http`
/// cannot depend on this crate (it pulls `syn`), so it carries its own copy of the
/// grammar and pins it against this one in a dev-dependency test that reads the
/// module's items by path. A rule whose runtime copy needs only a function to
/// call — `queue_name` — stays private and re-exports that function flat
/// (`framework.md`, *written twice and pinned once*).
pub mod versioning;

pub use args::{
    WrittenKeys, duplicate_argument, key_as_written, missing_argument, needs_a_value,
    one_role_per_method, require_str_lit, site, takes_value, unknown_argument, unknown_value,
    unmatched_meta,
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
pub use inject::{
    InjectableBody, LayerDeps, build_injectable_body, dependencies_method, dependency_names_method,
    forwarded_arg_idents, forwarded_idents, from_container_method, from_scope_method,
    injected_keyed_method, injected_keys_with_layers, injected_method,
    injected_methods_with_layers, injected_names_method, injected_names_with_layers, layer_deps,
    mixed_site_ident, normalize_forwarded_args, optional_dependencies_method,
};
pub use job::{
    JobDecorator, JobKey, TRANSACTIONAL, job_argument_needs_a_value, job_key, job_keys,
    job_returns_a_result, job_transaction, transactional_value, unread_job_key,
};
pub use mount::reject_path;
pub use pair::{DecoratorPair, parse_provider_host, provider_residency};
pub use posture::{
    ID_ARG_UNSUPPORTED_BECAUSE, Posture, PostureRules, at_most_one_authorize,
    posture_contradiction, posture_key_unsupported, posture_required,
};
pub use queue_name::{invalid_queue_name, is_valid_queue_name};
pub use replicas::{REPLICAS, replicas_default, replicas_value};
pub use root::reroot;
pub use route_path::RoutePath;
pub use specs::{force_guard_typeids, scoped_specs};
pub use ty::{
    HostBorrow, PipeWrapper, await_if_async, concrete_signature, generic_args, impl_self_ident,
    last_segment_ident, nth_generic_type, payload_arg_type, pipe_wrapper, returns_unit,
    shared_receiver, type_label,
};
pub use ungrouped::ungrouped_expr;
pub use versioning::{Edge, VersionAnswer};
