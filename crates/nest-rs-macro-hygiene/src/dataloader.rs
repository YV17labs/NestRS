//! `#[dataloader]` — one batched async-graphql `Loader` per method, routed
//! through `nest-rs-graphql`'s re-export.

use std::collections::HashMap;

use nest_rs::core::injectable;
use nest_rs::graphql::dataloader;

/// A batch failure: `Loader::Error` must be `Clone`, which `std::io::Error` is not.
#[derive(Debug, Clone)]
pub struct HygieneBatchError;

#[injectable]
#[derive(Default)]
pub struct HygieneDataloaders;

#[dataloader]
impl HygieneDataloaders {
    /// The fallible arm: `Loader::Error` is taken from the `Result`.
    async fn labels(&self, keys: &[String]) -> Result<HashMap<String, String>, HygieneBatchError> {
        Ok(keys.iter().map(|k| (k.clone(), k.to_uppercase())).collect())
    }

    /// The infallible arm: a bare map emits `::std::convert::Infallible`.
    async fn counts(&self, keys: &[String]) -> HashMap<String, i32> {
        keys.iter().map(|k| (k.clone(), k.len() as i32)).collect()
    }
}
