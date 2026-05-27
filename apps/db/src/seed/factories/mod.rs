//! One factory per seeded entity. A factory owns its row shape, its demo set —
//! fixed where an app depends on the values, faker-generated otherwise — and the
//! SeaQuery insert that seeds it. [`run`](super::run) drives them in FK order.
pub mod org;
pub mod user;
