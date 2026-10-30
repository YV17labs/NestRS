//! `paginate` configures the `list` operation `#[crud]` generates; beside the
//! list query the block writes by hand (`orgs`), nothing reads it — refused,
//! never a bound dropped without a word.

use nest_rs::graphql::{crud, resolver};

#[resolver]
struct OrgsResolver;

#[crud(service = svc, paginate = none)]
impl OrgsResolver {
    #[query]
    #[public]
    async fn orgs(&self) -> Vec<String> {
        Vec::new()
    }
}

fn main() {}
