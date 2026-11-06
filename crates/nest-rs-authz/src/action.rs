//! Action verbs + compile-time markers for routes that name one as a type
//! argument (`#[authorize(Read, _)]`).

/// The verb a rule grants or denies. A rule pairs one of these with a subject
/// entity; [`Manage`](Self::Manage) is the wildcard a grant covers all four
/// concrete verbs with, and what a site asks for when it needs them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Action {
    /// Read/list the entity — the query-filter and response-mask layers.
    Read,
    /// Create a new instance of the entity.
    Create,
    /// Modify an existing instance.
    Update,
    /// Remove an instance.
    Delete,
    /// Every action: a grant on it matches each of the four, a site gated on it
    /// needs each of the four.
    Manage,
}

impl Action {
    /// The word a line names this action by.
    pub const fn as_str(self) -> &'static str {
        match self {
            Action::Read => "read",
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
            Action::Manage => "manage",
        }
    }

    /// The concrete actions, in the order a refusal names what they lack.
    pub(crate) const CONCRETE: [Action; 4] =
        [Action::Read, Action::Create, Action::Update, Action::Delete];

    /// The concrete actions a site gated on this one needs.
    pub(crate) const fn concrete(self) -> &'static [Action] {
        match self {
            Action::Read => &[Action::Read],
            Action::Create => &[Action::Create],
            Action::Update => &[Action::Update],
            Action::Delete => &[Action::Delete],
            Action::Manage => &Self::CONCRETE,
        }
    }
}

/// Lets a route name an [`Action`] as a type argument on stable Rust (enum
/// const generics still need nightly `adt_const_params`).
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
        for other in Action::CONCRETE {
            assert_ne!(Action::Manage, other);
        }
    }

    #[test]
    fn a_concrete_action_needs_itself_and_manage_needs_all_four() {
        for action in Action::CONCRETE {
            assert_eq!(action.concrete(), [action]);
        }
        assert_eq!(Action::Manage.concrete(), Action::CONCRETE);
    }
}
