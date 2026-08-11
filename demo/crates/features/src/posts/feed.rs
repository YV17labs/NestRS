use nest_rs::core::injectable;
use nest_rs::graphql::async_graphql::futures_util::stream::{self, Stream};
use tokio::sync::broadcast;

use super::entities::post::Post;

const CAPACITY: usize = 64;

#[injectable]
pub struct PostFeed {
    tx: broadcast::Sender<Post>,
}

impl Default for PostFeed {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(CAPACITY).0,
        }
    }
}

impl PostFeed {
    pub fn publish(&self, post: Post) {
        let _ = self.tx.send(post);
    }

    pub fn subscribe(&self) -> impl Stream<Item = Post> + use<> {
        stream::unfold(self.tx.subscribe(), |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(post) => return Some((post, rx)),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
    }
}
