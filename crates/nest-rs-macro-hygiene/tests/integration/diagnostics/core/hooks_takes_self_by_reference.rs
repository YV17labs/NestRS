//! A lifecycle hook borrows its host: `self: Arc<Self>` is refused quoting the
//! receiver written, not with what rustc says of the expansion's call.

use nest_rs::core::hooks;

struct Svc;

#[hooks]
impl Svc {
    #[on_module_init]
    async fn init(self: std::sync::Arc<Self>) {}
}

fn main() {}
