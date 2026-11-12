//! [`CloseCode`], the status a WebSocket Close frame carries (RFC 6455 §7.4),
//! and the frames a socket edge closes with.

use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as Coded;
use tokio_tungstenite::tungstenite::{Error, Message};

/// The status a WebSocket closes with (RFC 6455 §7.4): a variant for each
/// code 6.x named — §7.4.1's but the reserved `1004` and `1015`, and the IANA
/// registry's `1012` and `1013` — and any other carried as sent in
/// [`Reserved`](Self::Reserved), an application's `4000`–`4999` such as
/// graphql-transport-ws's `4409` among them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloseCode {
    /// 1000: the purpose the connection was opened for is fulfilled.
    Normal,
    /// 1001: an endpoint is going away — a server going down, a browser
    /// leaving the page.
    Away,
    /// 1002: the peer broke the protocol.
    Protocol,
    /// 1003: a message of a kind the endpoint does not accept, such as binary
    /// data where only text is understood.
    Unsupported,
    /// 1005: the Close frame carried no status; never sent in one.
    Status,
    /// 1006: the connection ended without a Close frame; never sent in one.
    Abnormal,
    /// 1007: a message whose data does not match its type, such as text that
    /// is not UTF-8.
    Invalid,
    /// 1008: a message that violates the endpoint's policy, when no other
    /// code fits.
    Policy,
    /// 1009: a message too big to process.
    Size,
    /// 1010: the client expected an extension the server did not negotiate.
    Extension,
    /// 1011: an unexpected condition kept the server from serving the
    /// connection.
    Error,
    /// 1012: the server is restarting.
    Restart,
    /// 1013: the server is overloaded; try again later.
    Again,
    /// Any other code, carried as sent.
    Reserved(u16),
}

impl From<u16> for CloseCode {
    fn from(code: u16) -> Self {
        match code {
            1000 => Self::Normal,
            1001 => Self::Away,
            1002 => Self::Protocol,
            1003 => Self::Unsupported,
            1005 => Self::Status,
            1006 => Self::Abnormal,
            1007 => Self::Invalid,
            1008 => Self::Policy,
            1009 => Self::Size,
            1010 => Self::Extension,
            1011 => Self::Error,
            1012 => Self::Restart,
            1013 => Self::Again,
            other => Self::Reserved(other),
        }
    }
}

impl From<CloseCode> for u16 {
    fn from(code: CloseCode) -> Self {
        match code {
            CloseCode::Normal => 1000,
            CloseCode::Away => 1001,
            CloseCode::Protocol => 1002,
            CloseCode::Unsupported => 1003,
            CloseCode::Status => 1005,
            CloseCode::Abnormal => 1006,
            CloseCode::Invalid => 1007,
            CloseCode::Policy => 1008,
            CloseCode::Size => 1009,
            CloseCode::Extension => 1010,
            CloseCode::Error => 1011,
            CloseCode::Restart => 1012,
            CloseCode::Again => 1013,
            CloseCode::Reserved(other) => other,
        }
    }
}

/// RFC 6455 §5.5: a control frame's payload is at most 125 bytes, the code
/// taking two of them.
const REASON_MAX: usize = 123;

/// §7.4.1 **1009 Message Too Big**: a message past the socket's cap.
const MESSAGE_TOO_BIG: &str = "a message is past the size this server takes";

/// §7.4.1 **1011 Internal Error**: the socket could not be read, its framing
/// broken or the connection gone.
const READ_FAILED: &str = "the connection could not be read";

/// The Close frame carrying `code` and `reason`, the reason cut on a character
/// boundary to what a control frame holds, so no sentence makes a frame the
/// peer must refuse.
pub fn close_frame(code: CloseCode, reason: &str) -> Message {
    let reason = &reason[..reason.floor_char_boundary(REASON_MAX)];
    Message::Close(Some(CloseFrame {
        code: Coded::from(u16::from(code)),
        reason: reason.into(),
    }))
}

/// The code and reason a socket whose read failed closes with: **1009** for a
/// message or a frame past its cap, which the protocol layer reports by type,
/// **1011** for anything else.
pub fn read_failed(error: &Error) -> (CloseCode, &'static str) {
    match error {
        Error::Capacity(_) => (CloseCode::Size, MESSAGE_TOO_BIG),
        _ => (CloseCode::Error, READ_FAILED),
    }
}

#[cfg(test)]
mod tests {
    use tokio_tungstenite::tungstenite::error::{CapacityError, ProtocolError};

    use super::*;

    #[test]
    fn every_u16_round_trips_through_close_code() {
        for code in 0..=u16::MAX {
            assert_eq!(u16::from(CloseCode::from(code)), code);
        }
        for (code, named) in [
            (1000, CloseCode::Normal),
            (1001, CloseCode::Away),
            (1009, CloseCode::Size),
            (1011, CloseCode::Error),
        ] {
            assert_eq!(CloseCode::from(code), named);
        }
        for application in [4400, 4401, 4403, 4409, 4429] {
            assert_eq!(
                CloseCode::from(application),
                CloseCode::Reserved(application),
                "graphql-transport-ws's {application} is carried as sent",
            );
        }
    }

    #[test]
    fn every_code_reaches_the_wire_as_its_number() {
        for code in [1000, 1001, 1009, 1011, 4408, 4409] {
            let Message::Close(Some(frame)) = close_frame(CloseCode::from(code), "bye") else {
                panic!("a Close frame");
            };
            assert_eq!(u16::from(frame.code), code);
            assert_eq!(frame.reason.as_str(), "bye");
        }
    }

    #[test]
    fn a_close_reason_is_cut_to_what_a_control_frame_carries_on_a_character_boundary() {
        let long = "é".repeat(100);
        let Message::Close(Some(frame)) = close_frame(CloseCode::Away, &long) else {
            panic!("a Close frame");
        };
        assert_eq!(
            frame.reason.len(),
            122,
            "the last whole character before the limit"
        );
        assert!(long.starts_with(frame.reason.as_str()));
    }

    #[test]
    fn a_read_past_the_cap_closes_1009_and_any_other_failure_1011() {
        let too_big = Error::Capacity(CapacityError::MessageTooLong {
            size: 512,
            max_size: 64,
        });
        assert_eq!(read_failed(&too_big), (CloseCode::Size, MESSAGE_TOO_BIG));
        let reset = Error::Protocol(ProtocolError::ResetWithoutClosingHandshake);
        assert_eq!(read_failed(&reset), (CloseCode::Error, READ_FAILED));
    }
}
