//! The framework's identifier contract — an id is a UUID **v7** — and the one
//! sentence a client reads when a value breaks it.

/// What a client is told when an id is not a UUID v7.
pub const UUID_V7_REQUIRED: &str = "id must be a UUID v7";
