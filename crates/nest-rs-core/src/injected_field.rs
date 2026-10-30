//! [`InjectedField`] — a host's `#[inject]` field, read by its type across
//! the decorator pair.

/// One `#[inject]` field of a host, keyed by its name: its declared type, which
/// `#[crud(service = <field>)]` reads its service off. `#[controller]` and
/// `#[resolver]` implement it per field, the key a hash of the field's name
/// both halves compute alike.
#[diagnostic::on_unimplemented(
    message = "`{Self}` injects no field by the name `#[crud(service = …)]` gives",
    label = "no `#[inject]` field of that name",
    note = "`service = <field>` names the host's `#[inject]` field holding the service, e.g. `#[inject] svc: Arc<UsersService>` beside `#[crud(service = svc)]`"
)]
pub trait InjectedField<const FIELD: u64> {
    /// The field's declared type.
    type Type;
}
