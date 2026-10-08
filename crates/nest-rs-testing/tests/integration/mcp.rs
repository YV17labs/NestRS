//! `mcp::PROTOCOL_VERSION` against the SDK that answers it.

use nest_rs_mcp::ProtocolVersion;
use nest_rs_testing::mcp;

/// rmcp accepts every older revision forever, so nothing else fails when the
/// driver falls behind.
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
