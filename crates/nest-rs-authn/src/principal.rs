//! [`PrincipalIdentity`] — what every principal says about itself: its audit
//! identity, the tenant it acts within, and the scopes its credential carries.
//!
//!
//! The framework records `actor_id` on the request span when authentication
//! succeeds, so every downstream event inherits it, and makes the tenant
//! ambient for the request and every job it pushes.

use std::fmt;
use std::marker::PhantomData;

/// Stable audit identifier of a principal — the value recorded as the
/// request span's `actor_id` field, and the subject an MCP session is bound
/// to. Return the subject the verified credential names: an access token's
/// `sub`, which RFC 9068 §2.2 requires, the client's own id when the client
/// acts on its own behalf.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is no principal: it names no subject",
    label = "names no subject",
    note = "an access token names its subject (RFC 9068 §2.2), which is what a principal says: \
            `#[derive(PrincipalIdentity)]` on the claims struct and `#[principal(actor)]` on the \
            field naming it"
)]
pub trait PrincipalIdentity {
    /// The subject the verified credential names, or `None` for a principal
    /// naming none, which a gated MCP endpoint opens no session for.
    fn actor_id(&self) -> Option<String>;

    /// The tenant this principal acts within, or `None` (the default) for one
    /// bound to no tenant.
    ///
    /// Unlike [`actor_id`](Self::actor_id), an authorization input: the
    /// guard makes it ambient for the request and every continuation of it,
    /// and every job the request pushes runs for it —
    /// [`current_tenant_id`](nest_rs_core::current_tenant_id) reads it back. Read it off the
    /// verified credential alone, a claim its issuer signed, never off what
    /// the caller sends beside it. An empty id names no tenant.
    fn tenant_id(&self) -> Option<String> {
        None
    }

    /// The OAuth scopes this credential was granted, or `None` when the
    /// principal is not scope-aware.
    ///
    ///
    /// `None` (the default: a session, an mTLS identity) means scope is not a
    /// dimension of this credential, so scoped rules apply in full; `Some(&[])`
    /// is an OAuth credential granted nothing, for which every scoped rule is
    /// withheld.
    ///
    /// Implement it on a resource server's claims type, reading `scope` (RFC
    /// 6749 §3.3, space-delimited) or `scp`. It reports what the credential
    /// carries and is not an authorization decision.
    fn scopes(&self) -> Option<&[String]> {
        None
    }
}

/// An `actor` or `tenant` field the claims derive reads, for the role `R`,
/// held so the compiler can be asked whether its type is optional: called on
/// `&&IdentifierProbe`, an `Option<T>` answers through the first impl below,
/// autoref's first step, any other printing type through the second, and a
/// type that prints nothing reaches its role's refusal last.
pub struct IdentifierProbe<'a, T: ?Sized, R>(pub &'a T, pub PhantomData<R>);

impl<T: ?Sized, R> Clone for IdentifierProbe<'_, T, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized, R> Copy for IdentifierProbe<'_, T, R> {}

/// The role `#[principal(actor)]` gives a field.
pub struct Actor;

/// The role `#[principal(tenant)]` gives a field.
pub struct Tenant;

/// What [`IdentifierProbe`] answers: the text the field's value prints, or
/// `None` for an optional field holding none.
pub trait IdentifierAnswer {
    /// The field's value as an identifier.
    fn identifier(&self) -> Option<String>;
}

impl<T: fmt::Display, R> IdentifierAnswer for &IdentifierProbe<'_, Option<T>, R> {
    fn identifier(&self) -> Option<String> {
        self.0.as_ref().map(ToString::to_string)
    }
}

impl<T: fmt::Display + ?Sized, R> IdentifierAnswer for IdentifierProbe<'_, T, R> {
    fn identifier(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

/// An actor field's type, which nothing implements: what a field printing
/// nothing is refused through, in the derive's words.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be the principal's actor: `#[principal(actor)]` reads its field \
               as the text it prints",
    label = "prints no identifier",
    note = "an actor field holds a type implementing `Display` — a `String`, a `Uuid`, a number \
            — or an `Option` of one for a credential that may name no subject"
)]
pub trait ActorField {}

/// A tenant field's type, which nothing implements: what a field printing
/// nothing is refused through, in the derive's words.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be the principal's tenant: `#[principal(tenant)]` reads its field \
               as the text it prints",
    label = "prints no identifier",
    note = "a tenant field holds a type implementing `Display` — a `String`, a `Uuid`, a number \
            — or an `Option` of one for a credential bound to no tenant"
)]
pub trait TenantField {}

/// The last step of an actor's probe, reached by a field printing nothing.
pub trait ActorRefusal {
    /// The field's type.
    type Field: ?Sized;
    /// Never answers: its bound refuses the field.
    fn identifier(self) -> Option<String>
    where
        Self::Field: ActorField;
}

impl<T: ?Sized> ActorRefusal for IdentifierProbe<'_, T, Actor> {
    type Field = T;
    fn identifier(self) -> Option<String>
    where
        Self::Field: ActorField,
    {
        None
    }
}

/// The last step of a tenant's probe, reached by a field printing nothing.
pub trait TenantRefusal {
    /// The field's type.
    type Field: ?Sized;
    /// Never answers: its bound refuses the field.
    fn identifier(self) -> Option<String>
    where
        Self::Field: TenantField;
}

impl<T: ?Sized> TenantRefusal for IdentifierProbe<'_, T, Tenant> {
    type Field = T;
    fn identifier(self) -> Option<String>
    where
        Self::Field: TenantField,
    {
        None
    }
}

/// A scopes field's type: a list of scopes, empty when the credential grants
/// none.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is no scope list: `#[principal(scopes)]` reads its field as the scopes the \
               credential grants",
    label = "not a list of scopes",
    note = "a scopes field holds a `Vec<String>`, empty when the credential grants none — never \
            an `Option`, which would make a credential sent without its `scope` claim one no \
            scoped rule applies to"
)]
pub trait ScopeList {
    /// The scopes, as a slice.
    fn scopes(&self) -> &[String];
}

impl<T: AsRef<[String]> + ?Sized> ScopeList for T {
    fn scopes(&self) -> &[String] {
        self.as_ref()
    }
}

/// `field`'s scopes — what the derive's `scopes` reads, refusing a field that
/// holds no list through [`ScopeList`]'s words.
pub fn scope_list<L: ScopeList + ?Sized>(field: &L) -> &[String] {
    field.scopes()
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use super::{Actor, IdentifierAnswer as _, IdentifierProbe};

    /// The expansion's call, written once so every test asks at the depth
    /// the derive emits.
    macro_rules! probe {
        ($field:expr) => {
            (&&IdentifierProbe(&$field, PhantomData::<Actor>)).identifier()
        };
    }

    #[test]
    fn a_present_field_answers_the_text_it_prints() {
        assert_eq!(probe!(String::from("ada")), Some("ada".to_owned()));
        assert_eq!(probe!("acme"), Some("acme".to_owned()));
        assert_eq!(probe!(42_u64), Some("42".to_owned()));
    }

    #[test]
    fn an_optional_field_answers_its_value_or_none() {
        assert_eq!(probe!(Some(String::from("ada"))), Some("ada".to_owned()));
        assert_eq!(probe!(Some(7_u32)), Some("7".to_owned()));
        assert_eq!(probe!(None::<String>), None);
    }

    /// A type alias is the type it names, so an aliased `Option` answers as
    /// one: the probe reads the type, never its spelling.
    #[test]
    fn an_aliased_option_is_read_as_an_option() {
        type OrgId = Option<u32>;
        let absent: OrgId = None;
        assert_eq!(probe!(absent), None);
    }
}
