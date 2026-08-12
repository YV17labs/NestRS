//! Every field of the server's identity, refused where a host writes it.
//!
//! `McpIdentity` carries more than the `name`/`title` pair a host declares:
//! `description`, `website_url`, `icons` and `instructions` are real fields an
//! app sets, and one endpoint reports one of each however many features share
//! it. So a host reaching for one has not made a typo — the key exists, it is
//! declared where the whole surface is visible — and the answer names that seam
//! rather than listing spellings. This is the snapshot that fails when a field
//! is added to the identity and its answer is not.
//!
//! The host structs carry nothing but `#[mcp]`: the argument is refused before
//! anything is emitted, so nothing else can bury the sentence.

use nest_rs_mcp::mcp;

#[mcp(description = "What this server is")]
struct DescribedTool;

#[mcp(website_url = "https://example.test")]
struct LinkedTool;

#[mcp(icons = [])]
struct IconicTool;

#[mcp(instructions = "Ask before writing")]
struct InstructiveTool;

fn main() {}
