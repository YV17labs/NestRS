//! A marker names one instance, so it takes no type parameter: `Cache<T>`
//! would be as many instances as types, all reading one name.

use nest_rs::core::instance;

#[instance]
pub struct Cache<T>;

fn main() {}
