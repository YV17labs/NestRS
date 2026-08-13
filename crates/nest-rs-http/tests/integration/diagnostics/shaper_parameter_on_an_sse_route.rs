//! The `#[authorize]` refusal keys on the attribute; this one keys on the
//! parameter, because the attribute is not the only way to reach the shaper.
//!
//! `#[public]` beside a hand-written `Authorize<A, E>` is the sanctioned
//! "public reads" spelling, and `Bind<A, S>` arms the shaper too. On a stream
//! either one is waved through unmasked — the mask reads `text/event-stream` as
//! an opaque body and returns it untouched — while the route records itself as
//! masked and the document says field-level authorization applies.

// The macro bails before emitting the item, so every import is unused by the
// time rustc looks — noise the snapshot should not carry.
#![allow(unused_imports)]

use nest_rs_http::{SseEvent, SseStream, controller, futures_util::stream, routes};

struct Read;
struct Post;
struct Authorize<A, E>(std::marker::PhantomData<(A, E)>);

#[controller(path = "/feed")]
struct FeedController;

#[routes]
impl FeedController {
    #[sse("/ticks")]
    #[public]
    async fn ticks(&self, _proof: Authorize<Read, Post>) -> SseStream {
        SseStream::new(stream::iter([SseEvent::message("tick")]))
    }
}

fn main() {}
