//! String-keyed injection was removed in 7.0: two instances of one type are two
//! types. A 6.x `#[inject(key = "…")]` is told what replaced it, not that `key`
//! is unknown.

use nest_rs::core::injectable;

#[injectable]
#[derive(Default)]
struct Pool;

#[injectable]
struct Repo {
    #[inject(key = "cache")]
    pool: std::sync::Arc<Pool>,
}

fn main() {}
