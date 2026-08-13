//! `#[http_code]`, `#[redirect]` and `#[response_header]` all shape a response
//! that completes. A stream does not, so one sentence refuses the family rather
//! than three refusing a key each — a fourth response decorator inherits it.

// The macro bails before emitting the item, so every import is unused by the
// time rustc looks — noise the snapshot should not carry.
#![allow(unused_imports)]

use nest_rs_http::{SseEvent, SseStream, controller, futures_util::stream, http_code, routes};

#[controller(path = "/feed")]
struct FeedController;

#[routes]
impl FeedController {
    #[sse("/ticks")]
    #[public]
    #[http_code(201)]
    async fn ticks(&self) -> SseStream {
        SseStream::new(stream::iter([SseEvent::message("tick")]))
    }
}

fn main() {}
