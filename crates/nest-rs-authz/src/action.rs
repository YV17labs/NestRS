//! Action verbs + compile-time markers for routes that name one as a type
//! parameter (`Authorize<Read, _>`).

/// The verb a rule grants or denies. A rule pairs one of these with a subject
/// entity; [`Manage`](Self::Manage) is the wildcard that matches all four
/// concrete verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Read/list the entity — the query-filter and response-mask layers.
    Read,
    /// Create a new instance of the entity.
    Create,
    /// Modify an existing instance.
    Update,
    /// Remove an instance.
    Delete,
    /// The wildcard grant — matches every action.
    Manage,
}

/// Lets a route name an [`Action`] as a type argument on stable Rust (enum
/// const generics still need nightly `adt_const_params`).
///
/// The `on_unimplemented` note names the closed set, because the bound fires
/// wherever an action is a type argument — `Authorized<A, E>`, `#[authorize]`,
/// any generic over one — and rustc's default ("the trait is not implemented")
/// leaves the reader guessing what belongs there. It stays generic on purpose:
/// the one caller whose parameter *order* is the usual cause lives in
/// `nest-rs-seaorm`, a crate this one may not depend on, so that story is told
/// by `CrudService`'s own note instead.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an action marker",
    label = "expected an action marker here",
    note = "actions are `Create`, `Read`, `Update`, `Delete` and `Manage`."
)]
pub trait ActionMarker: Send + Sync + 'static {
    /// The runtime [`Action`] this type marker stands for.
    const ACTION: Action;
}

macro_rules! action_marker {
    ($name:ident) => {
        #[doc = concat!("Type marker for [`Action::", stringify!($name), "`].")]
        #[derive(Debug, Clone, Copy)]
        pub struct $name;
        impl ActionMarker for $name {
            const ACTION: Action = Action::$name;
        }
    };
}

action_marker!(Read);
action_marker!(Create);
action_marker!(Update);
action_marker!(Delete);
action_marker!(Manage);

#[cfg(test)]
mod tests {
    use super::*;

    // A route names an action as a type parameter (`Authorize<Read, _>`); the
    // marker must reflect the matching variant.
    #[test]
    fn each_marker_maps_to_its_action_variant() {
        assert_eq!(Read::ACTION, Action::Read);
        assert_eq!(Create::ACTION, Action::Create);
        assert_eq!(Update::ACTION, Action::Update);
        assert_eq!(Delete::ACTION, Action::Delete);
        assert_eq!(Manage::ACTION, Action::Manage);
    }

    #[test]
    fn manage_is_distinct_from_every_other_action() {
        for other in [Action::Read, Action::Create, Action::Update, Action::Delete] {
            assert_ne!(Action::Manage, other);
        }
    }
}
