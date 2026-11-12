//! `#[queue]`'s two values of the wrong kind, each refused at itself and opening
//! with the decorator and the key: a name that is not a string, and a job that
//! is not a type. syn's `expected string literal` and its list of the tokens a
//! type may start with named neither.

use nest_rs::queue::queue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmailCommand {
    to: String,
}

#[queue(name = emails, job = EmailCommand)]
struct EmailsQueue;

#[queue(name = "reports", job = "EmailCommand")]
struct ReportsQueue;

fn main() {}
