//! One decorator, one item shape — the host half of the five provider-hosted
//! pairs. Each of them names `#[injectable]` when it lands on a struct; the
//! sentence coming back the other way answered `expected struct`, which is the
//! phrasing the rule exists to forbid. `#[injectable]` is the one host that
//! cannot name *the* sibling — it has five — so it names the family.

use nest_rs_core::injectable;

struct DemoProvider;

#[injectable]
impl DemoProvider {
    fn tick(&self) {}
}

fn main() {}
