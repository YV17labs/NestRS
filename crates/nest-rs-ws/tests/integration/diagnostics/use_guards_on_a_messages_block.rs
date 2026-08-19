//! The WS member of the host-scope-layer family — one sentence, four edges.
//! This edge said nothing at all before: `use_guards` is no standalone
//! attribute macro, so it reached rustc as `cannot find attribute`.

use nest_rs_ws::{gateway, messages};

struct AllowAll;

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages]
#[use_guards(AllowAll)]
impl ChatGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        String::new()
    }
}

fn main() {}
