//! `#[derive(PrincipalIdentity)]`, re-exported by `nest-rs-authn`.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod principal_identity;

/// Implements `PrincipalIdentity` and `AmbientValue` for a claims struct,
/// reading each role off the field `#[principal(…)]` marks.
///
/// `#[principal(actor)]` is required, `#[principal(tenant)]` and
/// `#[principal(scopes)]` optional, each role on one field, a field playing
/// several. Refuses a generic struct, an enum and a union.
#[proc_macro_derive(PrincipalIdentity, attributes(principal))]
pub fn derive_principal_identity(input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(principal_identity::principal_identity(input.into())).into()
}
