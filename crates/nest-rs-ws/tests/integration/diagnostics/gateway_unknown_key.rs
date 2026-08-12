//! `#[gateway]` accepts three keys and no others. The refusal names all three,
//! so a developer who reached for the wrong word learns the whole grammar from
//! the error instead of from the macro's source — and adding `version` without
//! updating this sentence would leave the diagnostic describing a grammar the
//! decorator no longer has.

use nest_rs_ws::gateway;

#[gateway(path = "/ws", prefix = "/v1")]
struct BadGateway;

fn main() {}
