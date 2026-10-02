//! Suite root: one module per family joined.
//!
//! Each module derives its members from the source, derives what the suites
//! cover from the suites, and fails on the difference. A module here is not a
//! test of the framework's behaviour — it is a test of whether that behaviour
//! is covered anywhere, which is the one question no individual suite can ask
//! about itself.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod acls;
mod blinds;
mod decodes;
mod dependencies;
mod docs;
mod durations;
mod edges;
mod entries;
mod env_names;
mod events;
mod filters;
mod grammars;
mod guards;
mod keys;
mod mirrors;
mod naming;
mod panics;
mod paths;
mod queue_capabilities;
mod seams;
mod shapes;
mod snapshots;
mod targets;
mod transports;
mod umbrella;
mod units;
mod upgrading;

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

/// Write each `(path, text)` of `tree` below `root`, creating its folders — the
/// planted trees a join's own verdict is taken over, so a join is proved on a
/// tree whose answer is written beside it rather than only on the real one,
/// where a reading that is wrong the same way twice passes.
///
/// Here because several joins plant one.
pub(crate) fn plant(root: &std::path::Path, tree: &[(&str, &str)]) {
    for (file, text) in tree {
        let path = root.join(file);
        let folder = path.parent().expect("a planted file sits in a folder");
        std::fs::create_dir_all(folder).expect("the scratch tree is writable");
        std::fs::write(&path, text).expect("the scratch tree is writable");
    }
}

/// A name a join reads by its **spelling**, and the place in the source it
/// reads it at — what the `blinds` join refuses to let a construction hide.
///
/// A syntactic join sees a name only where the source writes it, so a
/// construction that writes the same item under another name, or emits it from
/// a `macro_rules!` the join does not expand, takes a member out of the join's
/// population while the join stays green. Each join declares what it reads this
/// way in a `followed()` beside the code that reads it; the `blinds` join gathers
/// every declaration and refuses the constructions that would hide one.
///
/// Here because every join declares one — `main.rs` holds what the siblings
/// share.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Followed {
    pub(crate) name: String,
    pub(crate) at: At,
    /// The path segment a `use` reaches the name through, when the name alone
    /// is shared with unrelated items — `error` for `std::error::Error`, which
    /// `async_graphql::Error` is not. Empty for a name nothing else wears.
    pub(crate) through: &'static [&'static str],
    /// Whether the join reads the name inside a `macro_rules!` transcriber too.
    pub(crate) in_transcribers: bool,
}

/// Where a followed name is read, which decides what can hide it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum At {
    /// An attribute, by the last segment of its path or an ident in its
    /// arguments — `#[module]`, `#[derive(Error)]`.
    Attribute,
    /// A trait, where it is implemented or bounded — `impl Config for`.
    Trait,
    /// A type, wherever its name is written — `Capability::Throttle`,
    /// `QueueBackend::new`, `DecoratorPair { … }`, `&ConfigService`.
    Type,
    /// A type the join reads by its spelling and whose `type` aliases it
    /// resolves by name — `Box`, which a `type X = Box<dyn …>` an intake's bound
    /// names — so only a rename hides it.
    Aliased,
    /// A function or a macro, where it is called — `panic_message(…)`,
    /// `operation_span!(…)`.
    Call,
    /// A function, where it is declared — `pub fn for_root`.
    Declaration,
    /// A crate, read through the shared import resolver: a rename is followed,
    /// so only what the resolver cannot follow hides it — a glob of its items.
    Crate,
    /// A macro argument's key whose literal value a join reads — `target:`.
    Key,
}

impl Followed {
    fn new(at: At, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            at,
            through: &[],
            in_transcribers: false,
        }
    }

    pub(crate) fn attribute(name: impl Into<String>) -> Self {
        Self::new(At::Attribute, name)
    }

    pub(crate) fn implemented(name: impl Into<String>) -> Self {
        Self::new(At::Trait, name)
    }

    pub(crate) fn type_(name: impl Into<String>) -> Self {
        Self::new(At::Type, name)
    }

    pub(crate) fn aliased(name: impl Into<String>) -> Self {
        Self::new(At::Aliased, name)
    }

    pub(crate) fn call(name: impl Into<String>) -> Self {
        Self::new(At::Call, name)
    }

    pub(crate) fn declaration(name: impl Into<String>) -> Self {
        Self::new(At::Declaration, name)
    }

    pub(crate) fn krate(name: impl Into<String>) -> Self {
        Self::new(At::Crate, name)
    }

    pub(crate) fn key(name: impl Into<String>) -> Self {
        Self::new(At::Key, name)
    }

    /// Only the item a `use` reaches through one of `owners`.
    pub(crate) fn through(mut self, owners: &'static [&'static str]) -> Self {
        self.through = owners;
        self
    }

    /// The join reads the name inside a `macro_rules!` transcriber too.
    pub(crate) fn read_in_transcribers(mut self) -> Self {
        self.in_transcribers = true;
        self
    }
}
