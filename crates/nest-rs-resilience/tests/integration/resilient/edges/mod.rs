//! A decorated method of every impl half that takes one, called through the
//! framework's own path and attempted again within that one call: a refusal
//! answered `503` by every edge serving a decorated handler (`routes`,
//! `messages`, `operations`, `tools`), the batch of a `#[dataloader]` host,
//! whose error the loader clones for each key (`dataloader`), and a refusal
//! where no client reads a `503` — a job, a tick, a listener, an indicator, a
//! hook — filed as that unit's failure with what the framework does next.

use super::*;

mod dataloader;
mod hooks;
mod indicators;
mod listeners;
mod messages;
mod operations;
mod processor;
mod routes;
mod scheduled;
mod tools;
