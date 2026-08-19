//! `id_arg = argument` is refused **by name**, and it is the third key of the
//! `#[authorize]` grammar to get one.
//!
//! It was refused at exactly one of the three sites that cannot express it, and
//! there through the `bind` helper — so a developer who wrote `id_arg` and never
//! wrote `bind` read *"`bind = Service` is not available on HTTP — and neither
//! is `id_arg`…"*. `CLAUDE.md`: "Refusals are shared, not per key. One helper,
//! one sentence, every key it covers, **one trybuild snapshot per site**."

use nest_rs_ws::{gateway, messages};

struct Update;
struct ChatService;
mod messages_entity {
    pub struct Entity;
}

#[gateway(path = "/ws")]
struct ChatGateway;

#[messages]
impl ChatGateway {
    #[subscribe_message("send")]
    #[authorize(Update, id_arg = subject_id)]
    async fn send(&self) {}
}

fn main() {}
