//! `mcp::PROTOCOL_VERSION` against the SDK that answers it.

use nest_rs_mcp::ProtocolVersion;
use nest_rs_testing::mcp;

/// The driver pinned `2024-11-05` — the oldest of five — for four revisions,
/// and nothing said so: rmcp accepts it forever, so every MCP suite in both
/// workspaces passed while negotiating a handshake no current client performs.
/// A shared constant was never the guard against that; this is.
#[test]
fn the_driver_negotiates_the_sdk_latest() {
    assert_eq!(
        mcp::PROTOCOL_VERSION,
        ProtocolVersion::LATEST.as_str(),
        "rmcp moved its LATEST: bump `mcp::PROTOCOL_VERSION` to match, and check \
         whether the new revision still uses the `mcp-session-id` session model \
         `open_session` implements (SEP-2567 retires it at 2026-07-28)",
    );
}
