//! Shared helpers for nestrs decorator macros.
//!
//! A procedural macro must live in a `proc-macro = true` crate, which can
//! export nothing but macros — so the token-building logic every decorator
//! shares (`#[injectable]`-style construction, the `from_container`
//! constructor, the `Discoverable::dependencies` list) lives here, in a plain
//! library crate that each `nestrs-*-macros` crate depends on. Third-party
//! decorator crates can depend on it too.

mod args;
mod crud;
mod inject;
mod ty;

pub use args::parse_named_str_arg;
pub use crud::{parse_crud_args, singular_of, CrudConfig, Paginate};
pub use inject::{
    build_injectable_body, dependencies_method, dependency_names_method, forwarded_arg_idents,
    forwarded_idents, from_container_method, injected_keys_expr, injected_method,
    optional_dependencies_method, InjectableBody,
};
pub use ty::{impl_self_ident, nth_generic_type};
