//! MCP bindings (feature `mcp`) — the tool-host analog of [`crate::graphql`].
//!
//! [`McpAbilityBridge`] is the endpoint's per-operation guard: it authenticates
//! each `/mcp` request with the same chain controllers use and installs the
//! caller's ambient [`Ability`](crate::Ability) for the operation's duration.
//! [`authorize`] is the class-level gate and [`masked_value_for`] masks an
//! operation's return value.
//!
//! A host does not call the last two directly: `#[authorize(Action, Entity)]`
//! beside a `#[tool]` / `#[prompt]` makes `#[tools]` emit both, exactly as
//! `#[operations]` does on GraphQL and `#[routes]` on HTTP.
//!
//! ```ignore
//! #[tools]
//! impl UsersTool {
//!     /// List the people the caller may see.
//!     #[tool]
//!     #[authorize(Read, users::Entity)]
//!     async fn list_people(&self) -> Result<Json<Vec<User>>, McpError> {
//!         // gate + response masking are emitted by the macro
//!     }
//! }
//! ```
//!
//! # Why these read the *ambient* ability
//!
//! GraphQL's gate takes a `&Context` because async-graphql threads one through
//! every resolver. An MCP operation is handed no such value — rmcp dispatches it
//! on its own task — so the caller's ability travels the way every other
//! ambient-state consumer on this transport reads it: the endpoint's operation
//! guard installs it in its `around`, and these read it back with
//! [`current_ability`](crate::current_ability). Same decision, same guard, one
//! less parameter.
//!
//! Data-coupled bindings live in `nest_rs_seaorm::mcp` (`McpDataContext`).

mod authorize;
mod bridge;
mod mask;

pub use authorize::authorize;
pub use bridge::McpAbilityBridge;
pub use mask::masked_value_for;
