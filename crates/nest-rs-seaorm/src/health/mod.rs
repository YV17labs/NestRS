//! The pool's indicator (`health` feature), which the module opening the pool
//! attaches from its `register`.

mod indicator;

pub(crate) use indicator::register;
