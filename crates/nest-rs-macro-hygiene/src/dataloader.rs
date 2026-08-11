//! `#[dataloader]` — one batched async-graphql `Loader` per method.
//!
//! Its expansion names `async_graphql::dataloader::Loader` and the loader
//! registry, both routed through `nest-rs-graphql`'s re-export; a resolver crate
//! declares neither. The *intended* use runs each batch through `Repo`, which is
//! why this decorator reads as needing a data layer — but that is the caller's
//! business, not the macro's: it reads a key type off the argument and a value
//! type off the return, and emits the same tokens either way. So it belongs
//! here, where the manifest is the assertion.

use std::collections::HashMap;

use nest_rs::core::injectable;
use nest_rs::graphql::dataloader;

/// A batch failure. `Loader::Error` is bound `Send + Clone + 'static` — one
/// error is handed to every caller waiting on the batch — so a service's error
/// type has to be `Clone` to be loadable. Spelled out here rather than reached
/// for from `std`, which is exactly why: `std::io::Error` is not `Clone`.
#[derive(Debug, Clone)]
pub struct HygieneBatchError;

/// Minimal batch host.
#[injectable]
#[derive(Default)]
pub struct HygieneLoaders;

#[dataloader]
impl HygieneLoaders {
    /// The fallible form: the error type is taken from the `Result`, so the
    /// generated `Loader::Error` is this one.
    async fn labels(&self, keys: &[String]) -> Result<HashMap<String, String>, HygieneBatchError> {
        Ok(keys.iter().map(|k| (k.clone(), k.to_uppercase())).collect())
    }

    /// The infallible form: a bare map makes the expansion substitute
    /// `::std::convert::Infallible`, which is a different arm of the same
    /// emission and would otherwise go unexercised.
    async fn counts(&self, keys: &[String]) -> HashMap<String, i32> {
        keys.iter().map(|k| (k.clone(), k.len() as i32)).collect()
    }
}
