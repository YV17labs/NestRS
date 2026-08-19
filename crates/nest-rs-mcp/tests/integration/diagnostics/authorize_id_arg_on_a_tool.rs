//! `id_arg = argument` is refused **by name**, and it is the third key of the
//! `#[authorize]` grammar to get one.
//!
//! It was refused at exactly one of the three sites that cannot express it, and
//! there through the `bind` helper — so a developer who wrote `id_arg` and never
//! wrote `bind` read *"`bind = Service` is not available on HTTP — and neither
//! is `id_arg`…"*. `CLAUDE.md`: "Refusals are shared, not per key. One helper,
//! one sentence, every key it covers, **one trybuild snapshot per site**."

use nest_rs_mcp::{mcp, tools};

struct Update;
struct DemoService;
mod demo {
    pub struct Entity;
}

#[mcp(path = "/demo")]
#[derive(Clone, Default)]
struct DemoTool;

#[tools]
impl DemoTool {
    #[tool(description = "Answer with a constant.")]
    #[authorize(Update, id_arg = subject_id)]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("ok".into())
    }
}

fn main() {}
