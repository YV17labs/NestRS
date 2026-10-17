//! `#[use_filters]` beside a gateway is refused for good, naming
//! `#[use_exception_filters]`: an exception filter catching
//! `dyn Error + Send + Sync` is what replaces HTTP's untyped `Filter`.

use nest_rs::ws::gateway;

#[gateway(path = "/ws")]
#[use_filters(SomeFilter)]
struct BadGateway;

fn main() {}
