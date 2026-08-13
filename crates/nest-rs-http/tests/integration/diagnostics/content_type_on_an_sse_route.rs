//! An `#[sse]` route answers `text/event-stream`. A declared
//! `response_content_type` cannot change that — it can only make the published
//! document describe something the route never sends, which is the failure mode
//! a generated client discovers and the document does not.

// The macro bails before emitting the item, so every import is unused by the
// time rustc looks — noise the snapshot should not carry.
#![allow(unused_imports)]

use nest_rs_http::{SseEvent, SseStream, controller, futures_util::stream, routes};

#[controller(path = "/feed")]
struct FeedController;

#[routes]
impl FeedController {
    #[sse("/ticks")]
    #[public]
    #[api(response_content_type = "application/json")]
    async fn ticks(&self) -> SseStream {
        SseStream::new(stream::iter([SseEvent::message("tick")]))
    }
}

fn main() {}
