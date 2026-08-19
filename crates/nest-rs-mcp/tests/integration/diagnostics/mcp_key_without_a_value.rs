//! One of the four refusals a `key = value` grammar owes, pinned where the
//! compiler says it. `CLAUDE.md`: "Refusals are shared, not per key. One
//! helper, one sentence, every key it covers, **one trybuild snapshot per
//! site**."

use nest_rs_mcp::mcp;

#[mcp(path)]
#[derive(Clone, Default)]
struct DemoTool;

fn main() {}
