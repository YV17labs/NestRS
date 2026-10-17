//! `#[module]` submits a `static` descriptor, which cannot be generic: a module
//! generic over an instance is refused naming the hand-written form, rather
//! than with rustc's missing-generics error from inside the expansion.

use nest_rs::core::{Instance, module};

#[module]
pub struct CacheModule<I: Instance>(std::marker::PhantomData<I>);

fn main() {}
