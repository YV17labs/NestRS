//! [`HygieneOperator`] — what a guard attaches, read back through a `Ctx<T>`
//! parameter on every edge.

use nest_rs::core::AmbientValue;

/// The caller a guard attached, as each edge's `Ctx<T>` reads it.
#[derive(Clone)]
pub struct HygieneOperator(pub String);

impl AmbientValue for HygieneOperator {}
