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
mod args;
mod casing;
mod crud;
mod inject;
mod ty;

pub use args::{parse_named_str_arg, require_str_lit};
pub use casing::{pascal_case, snake_case};
pub use crud::{CrudConfig, Paginate, parse_crud_args, singular_of};
pub use inject::{
    InjectableBody, build_injectable_body, dependencies_method, dependency_names_method,
    forwarded_arg_idents, forwarded_idents, from_container_method, from_scope_method,
    injected_keyed_method, injected_keys_expr, injected_keys_with_layers, injected_method,
    injected_method_with_layers, injected_names_method, layer_inject_keys,
    optional_dependencies_method,
};
pub use ty::{impl_self_ident, nth_generic_type};
