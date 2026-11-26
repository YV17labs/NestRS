//! Covers the macro crate's `src/expansion.rs`, what an accepted method expands
//! to: its body sent once or again as its attribute declares, on each receiver
//! and host it takes (`body`), when its arguments and what its body's tail
//! leaves drop, beside its undecorated twin (`dropped`), what its future holds
//! against its undecorated twin's, and a large one run on a worker of tokio's
//! default stack (`footprint`), the instrument written below it (`instrumented`),
//! and what its lines tell of the dependency's answer (`told`).

use super::*;

mod body;
mod dropped;
mod footprint;
mod instrumented;
mod told;
