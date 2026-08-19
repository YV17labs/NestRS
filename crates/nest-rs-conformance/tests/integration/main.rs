//! Suite root: one module per family joined.
//!
//! Each module derives its members from the source, derives what the suites
//! cover from the suites, and fails on the difference. A module here is not a
//! test of the framework's behaviour — it is a test of whether that behaviour
//! is covered anywhere, which is the one question no individual suite can ask
//! about itself.

mod edges;
mod env_names;
mod events;
mod filters;
mod guards;
mod panics;
mod seams;
mod shapes;
mod targets;
mod umbrella;
mod units;
