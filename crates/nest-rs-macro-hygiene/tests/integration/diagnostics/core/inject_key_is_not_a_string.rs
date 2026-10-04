//! A key is the name a keyed provider is registered under, so it is a string
//! literal. Anything else is refused naming the decorator and the key, where
//! syn's `expected string literal` named neither.

use nest_rs::core::injectable;

#[injectable]
#[derive(Default)]
struct Pool;

#[injectable]
struct Repo {
    #[inject(key = 42)]
    pool: std::sync::Arc<Pool>,
}

fn main() {}
