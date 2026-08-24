//! Suite root: one module per family joined.
//!
//! Each module derives its members from the source, derives what the suites
//! cover from the suites, and fails on the difference. A module here is not a
//! test of the framework's behaviour — it is a test of whether that behaviour
//! is covered anywhere, which is the one question no individual suite can ask
//! about itself.

mod canon;
mod docs;
mod edges;
mod env_names;
mod events;
mod filters;
mod grammars;
mod guards;
mod naming;
mod panics;
mod seams;
mod shapes;
mod targets;
mod umbrella;
mod units;

/// The closed edge vocabulary (`architecture.md`), the only set a canonical
/// name may take its namespace from.
///
/// The one list in this suite that is **stated rather than derived**, and it
/// has to be: the vocabulary is closed by an owner decision recorded in prose,
/// so there is nothing in the tree to read it off — a crate named
/// `nest-rs-grpc` would prove only that someone wrote one. Opening an edge
/// therefore touches `architecture.md` and this line, which is the deliberate,
/// reviewed act the closure exists to require.
///
/// Here rather than in a join because two of them read it — `units`, to check
/// a unit name's namespace, and `naming`, to tell an edge adapter from a
/// module-root role file. A copy in either would have made the reviewed act
/// two lines, and a join reaching into a sibling for it is the shape
/// `CLAUDE.md` names the suite root for: "`main.rs` is the suite *root* …
/// `//!` + the `mod` list + the fixtures the siblings share (`crate::…`)".
/// `sources` is the wrong home for the mirror reason — its own `//!` says
/// every member list there is walked from the tree, never listed.
pub(crate) const EDGES: [&str; 7] = [
    "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
];
