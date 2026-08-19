//! The WS member of the mount-path family — same sentence, and the fact is
//! RFC 3986 §3.3's: a mount path is `path-absolute`, so it begins with `/`.

use nest_rs_ws::gateway;

#[gateway(path = "ws")]
pub struct ChatGateway;

fn main() {}
