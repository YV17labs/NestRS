//! Mirror tests for `src/guard.rs`; each file covers one of the guard's four
//! entries and is gated on the feature that compiles it.

#[cfg(feature = "graphql")]
mod graphql;
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "ws")]
mod ws;

/// The rules the running unit reads under, in words a test asserts on: whose
/// they are, and whether they hold the one grant only a visitor's fixture writes.
#[cfg(any(feature = "graphql", feature = "mcp", feature = "ws"))]
fn rules_read() -> String {
    let ability = nest_rs_authz::current_ability().expect("the guard attached an ability");
    let who = if ability.is_visitor() {
        "visitor"
    } else {
        "caller"
    };
    let delete = ability.can_class(
        nest_rs_authz::Action::Delete,
        std::any::TypeId::of::<crate::widget::Entity>(),
    );
    format!("{who} may {}", if delete { "delete" } else { "not delete" })
}
