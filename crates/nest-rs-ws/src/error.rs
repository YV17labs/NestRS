//! What the boot says of a gateway whose messages declare a posture its guards
//! contradict.

use thiserror::Error;

/// A gateway's posture, refused at the kernel's wiring step: the boot can read
/// what the impl half cannot, the app's pool.
#[derive(Debug, Error)]
pub(crate) enum GatewayPostureError {
    /// Messages no guard covers and none opens, every one of them.
    #[error(
        "messages no guard covers and none marks `#[public]` on the gateway at {path}: \
         {messages} — a message runs a guard or is open on purpose; bind a guard with \
         `#[use_guards(...)]` on the gateway or the message, or declare the app's global pool \
         (`GlobalGuards<(...)>` in the providers of the module the app boots), or mark a message \
         open on purpose with `#[public]`"
    )]
    Uncovered { path: String, messages: String },
    /// A gateway with no guard at its upgrade and no message to say so.
    #[error(
        "the gateway at {path} runs no guard at its upgrade and declares no message: its \
         connection hooks run for any caller, and no line says so — bind a guard with \
         `#[use_guards(...)]` on the gateway, or declare the app's global pool \
         (`GlobalGuards<(...)>` in the providers of the module the app boots)"
    )]
    Unopened { path: String },
    /// `#[public]` messages no caller without a credential reaches.
    #[error(
        "`#[public]` on {messages} of the gateway at {path} can open nothing: its upgrade runs \
         `{guard}`, so a caller with no credential never reaches a message — drop `#[public]`, \
         the guards covering a message deciding it (`#[authorize(Action, Subject)]` adds a gate), \
         or take that guard off the upgrade to open the gateway"
    )]
    PublicBehindGuard {
        path: String,
        messages: String,
        guard: &'static str,
    },
}
