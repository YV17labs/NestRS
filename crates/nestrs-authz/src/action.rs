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
