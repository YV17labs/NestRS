//! `#[public]` is a flag: its presence *is* the declaration, so an argument on
//! it is a compile error rather than something the expansion drops.
//!
//! One of the four sites the shared refusal covers — `nest_rs_codegen::take_flag_attr`
//! words it once, and the flag is `#[public]` at every edge because that is the
//! greppable authn/authz site `CLAUDE.md` reserves. Before the refusal,
//! `#[public(read_only)]` — plausible beside `#[authorize(Action, Entity)]`,
//! which does take arguments — shipped an ungated, unmasked operation with the
//! compiler silent.

use nest_rs_ws::{gateway, messages};

#[gateway(path = "/ws")]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("list")]
    #[public(read_only)]
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }
}

fn main() {}
