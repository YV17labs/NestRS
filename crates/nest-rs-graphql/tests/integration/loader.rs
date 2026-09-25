//! `#[dataloader]` — the batch a generated loader runs is the owner's method.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_graphql::async_graphql::dataloader::Loader;
use nest_rs_graphql::dataloader;

/// A trait whose method shares the batch's name, implemented for the `Arc` the
/// loader holds its owner in. Method syntax on that `Arc` finds this before it
/// derefs to the owner, so a loader calling `self.0.labels(..)` ran this body.
#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait LabelsOnArc {
    fn labels(&self, keys: &[i32]) -> HashMap<i32, String>;
}

impl<T> LabelsOnArc for Arc<T> {
    fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        keys.iter()
            .map(|key| (*key, "the trait on Arc".into()))
            .collect()
    }
}

#[injectable]
#[derive(Default)]
struct Shelf;

#[dataloader]
impl Shelf {
    async fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        keys.iter()
            .map(|key| (*key, format!("shelf {key}")))
            .collect()
    }
}

#[tokio::test]
async fn a_loader_runs_its_owners_method_and_not_a_trait_method_of_the_same_name() {
    let loaded = ShelfLabels(Arc::new(Shelf)).load(&[7]).await;
    let Ok(loaded) = loaded;
    assert_eq!(loaded.get(&7).map(String::as_str), Some("shelf 7"));
}
