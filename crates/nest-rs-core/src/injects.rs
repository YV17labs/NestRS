//! The service a host injects, reached by its type: `#[controller]` and
//! `#[resolver]` implement [`Injects<S>`] for each `#[inject] Arc<S>` field,
//! and `#[crud(service = S)]` delegates to the field through it.
//!
//! Keyed by the service's type, so an impl is as visible as the least visible
//! type in its header and a public host may inject a crate-private service; a
//! block naming its host through an alias, an import or another module reaches
//! it as the struct's own module does.

/// A host holding a `#[inject] Arc<S>` field.
#[diagnostic::on_unimplemented(
    message = "`{Self}` injects no `{S}`",
    label = "`#[crud]` delegates to the host's `{S}` field",
    note = "`#[crud(service = …)]` names the type of an `#[inject]` field of its host: add `#[inject] svc: Arc<{S}>` to `{Self}`"
)]
pub trait Injects<S: ?Sized> {
    /// The field holding `S`.
    fn injected(&self) -> &std::sync::Arc<S>;
}
