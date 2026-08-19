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
mod inject;
mod job;
mod mount;
mod pair;
mod posture;
mod root;
mod specs;
mod ty;
/// Public because the grammar it words has a *runtime* counterpart:
/// `nest-rs-http` cannot depend on this crate (it pulls `syn`), so it carries a
/// copy of `is_valid_version` and pins it against this one in a dev-dependency
/// test. Naming the module is what lets that test exist.
pub mod versioning;

pub use args::{
    duplicate_argument, key_as_written, missing_argument, needs_a_value, once, one_role_per_method,
    require_str_lit, role_name, unknown_argument, unknown_value, unmatched_meta,
};
pub use attrs::{reject_http_only_layers, take_flag_attr, take_path_list};
pub use capability::guard_capability_bounds;
pub use casing::{pascal_case, snake_case};
pub use crud::{
    CrudConfig, CrudOp, GeneratedOps, OpsSelection, Paginate, parse_crud_args, singular_of,
};
pub use inject::{
    InjectableBody, LayerDeps, build_injectable_body, dependencies_method, dependency_names_method,
    forwarded_arg_idents, forwarded_idents, from_container_method, from_scope_method,
    injected_keyed_method, injected_keys_with_layers, injected_method,
    injected_methods_with_layers, injected_names_method, injected_names_with_layers, layer_deps,
    mixed_site_ident, normalize_forwarded_args, optional_dependencies_method,
};
pub use job::{TRANSACTIONAL, job_argument_needs_a_value, job_transaction, transactional_value};
pub use mount::reject_path;
pub use pair::{DecoratorPair, parse_provider_host, provider_residency};
pub use posture::{
    ID_ARG_UNSUPPORTED_BECAUSE, Posture, PostureRules, at_most_one_authorize,
    posture_contradiction, posture_key_unsupported, posture_required,
};
pub use root::reroot;
pub use specs::{force_guard_typeids, scoped_specs};
pub use ty::{
    PipeWrapper, generic_args, impl_self_ident, last_segment_ident, must_be_async,
    nth_generic_type, payload_arg_type, pipe_wrapper, type_label,
};
pub use versioning::{Edge, VersionAnswer};
