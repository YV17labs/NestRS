//! [`HygieneOperator`] — what a guard attaches, read back through an `Ambient<T>`
//! parameter on every edge.

use nest_rs::core::AmbientValue;

/// The caller a guard attached, as each edge's `Ambient<T>` reads it.
#[derive(Clone)]
pub struct HygieneOperator(pub String);

impl AmbientValue for HygieneOperator {}
