//! `#[use_filters]` on a gateway would be a silent no-op (filters are not
//! bridged on WebSockets) — the macro rejects it and points to the HTTP home
//! where the layer actually runs.

use nest_rs_ws::gateway;

#[gateway(path = "/ws")]
#[use_filters(SomeFilter)]
struct BadGateway;

fn main() {}
