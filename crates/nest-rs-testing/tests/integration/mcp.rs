//! `mcp::PROTOCOL_VERSION` against the SDK that answers it.

use nest_rs_mcp::ProtocolVersion;
use nest_rs_testing::mcp;

/// rmcp accepts every older revision forever, so nothing else fails when the
/// driver falls behind.
#[test]
fn the_driver_negotiates_the_sdk_latest_handshake() {
    assert_eq!(
        mcp::PROTOCOL_VERSION,
        ProtocolVersion::LATEST_WITH_INITIALIZE.as_str(),
        "rmcp moved its LATEST_WITH_INITIALIZE: bump `mcp::PROTOCOL_VERSION` to match",
    );
}
