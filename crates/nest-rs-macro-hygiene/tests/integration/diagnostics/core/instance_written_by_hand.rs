//! A hand-written `Instance` would choose its name and its key — here the
//! default's, under a reserved word — and file nothing for the twin check.

use nest_rs::core::Instance;

pub struct Hand;

impl Instance for Hand {
    const NAME: &'static str = "queue";
    type Of<T: Send + Sync + 'static> = T;
}

fn main() {}
