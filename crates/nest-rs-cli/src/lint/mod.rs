//! The one naming rule of `architecture.md` that a path cannot derive.
//!
//! Every other rule is derivable from a path; the pairing between a vocabulary
//! file's stem and the types it declares is not. It is refused in one shape: a
//! stem that reaches **nothing** the file declares — a file named for a slot
//! rather than a subject.

mod finding;
mod scan;

pub use finding::Finding;
pub use scan::{Scan, scan};
