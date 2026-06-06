//! Action verbs + compile-time markers for routes that name one as a type
//! parameter (`Authorize<Read, _>`).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Read,
    Create,
    Update,
    Delete,
    /// CASL `manage` — matches every action.
    Manage,
}

/// Lets a route name an [`Action`] as a type argument on stable Rust (enum
/// const generics still need nightly `adt_const_params`).
pub trait ActionMarker: Send + Sync + 'static {
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
