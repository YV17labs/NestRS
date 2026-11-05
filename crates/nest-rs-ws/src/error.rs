//! What the boot says of a gateway whose messages declare a posture its guards
//! contradict.

use nest_rs_http::__private::NO_GUARD_REMEDY;
use thiserror::Error;

/// A gateway's posture, refused at the kernel's wiring step: the boot can read
/// what the impl half cannot, the app's pool.
#[derive(Debug, Error)]
pub(crate) enum GatewayPostureError {
    /// Messages no guard covers and none opens, every one of them.
    #[error(
        "messages no guard covers and none marks `#[public]` on the gateway at {path}: \
         {messages} — a message runs a guard or is open on purpose; {NO_GUARD_REMEDY}"
    )]
    Uncovered { path: String, messages: String },
    /// A gateway with no guard at its upgrade and no `#[public]` message to say so.
    #[error(
        "the gateway at {path} runs no guard at its upgrade and no message of it says \
         `#[public]`: any caller connects, its connection hooks run and what the server emits \
         reaches the socket, and no line says so — {NO_GUARD_REMEDY}"
    )]
    Unopened { path: String },
    /// `#[public]` messages no caller without a credential reaches, past a guard
    /// the gateway binds.
    #[error(
        "`#[public]` on {messages} of the gateway at {path} can open nothing: its upgrade runs \
         `{guard}`, so a caller with no credential never reaches a message — drop `#[public]`, \
         the guards covering a message deciding it (`#[authorize(Action, Subject)]` adds a gate), \
         or take that guard off the upgrade to open the gateway"
    )]
    PublicBehindGuard {
        path: String,
        messages: String,
        guard: String,
    },
    /// The same, past a guard of the app's pool, which runs at every upgrade.
    #[error(
        "`#[public]` on {messages} of the gateway at {path} can open nothing: the app's pool \
         runs `{guard}` at its upgrade, so a caller with no credential never reaches a message — \
         drop `#[public]`, the guards covering a message deciding it \
         (`#[authorize(Action, Subject)]` adds a gate); no gateway opens under the pool, so \
         opening this one means moving `{guard}` from the pool onto the hosts it should cover"
    )]
    PublicBehindPool {
        path: String,
        messages: String,
        guard: String,
    },
}
