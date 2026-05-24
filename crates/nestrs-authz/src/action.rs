//! The verbs an ability authorizes, plus the compile-time markers that let a
//! route name one as a type parameter of [`crate::Authorize`].

/// The verbs an ability can grant or deny. [`Action::Manage`] is the wildcard
/// that matches every other action (CASL's `manage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Read,
    Create,
    Update,
    Delete,
    /// Matches every action — the CASL `manage` wildcard.
    Manage,
}

/// A zero-sized marker for an [`Action`], so a route can declare the action it
/// requires as a type argument (`Authorize<Read, _>`) on stable Rust — enum
/// const generics still need nightly `adt_const_params`.
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
