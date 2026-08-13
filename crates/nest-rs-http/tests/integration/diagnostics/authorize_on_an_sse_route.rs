//! `#[authorize]` arms response masking, which reconciles the body against the
//! entity model. An event stream is no wire model, so the mask could only fail
//! closed at 500 on every request — the refusal names the pattern that does
//! work instead.

// The macro bails before emitting the item, so every import is unused by the
// time rustc looks — noise the snapshot should not carry.
#![allow(unused_imports)]

use nest_rs_http::{SseEvent, SseStream, controller, futures_util::stream, routes};

struct Post;

#[controller(path = "/feed")]
struct FeedController;

#[routes]
impl FeedController {
    #[sse("/ticks")]
    #[authorize(Read, Post)]
    async fn ticks(&self) -> SseStream {
        SseStream::new(stream::iter([SseEvent::message("tick")]))
    }
}

fn main() {}
