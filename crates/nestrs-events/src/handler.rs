use async_trait::async_trait;

#[async_trait]
pub trait EventHandler: Send + Sync + 'static {
    type Event: Clone + Send + 'static;

    async fn handle(&self, event: Self::Event);
}
