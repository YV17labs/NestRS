//! An entity's posture masks the reply against its rows, and an event stream is
//! no wire model to reconcile, so `#[authorize]` on an `#[sse]` route names a
//! subject no table holds — decided by the posture's type, after name
//! resolution — and the refusal names that remedy.

use nest_rs::authz::Read;
use nest_rs::http::{SseEvent, SseStream, controller, futures_util::stream, routes};

mod post {
    use nest_rs::seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Post")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "streamed_posts")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        #[expose]
        pub id: Uuid,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[controller(path = "/feed")]
struct FeedController;

#[routes]
impl FeedController {
    #[sse("/ticks")]
    #[authorize(Read, post::Entity)]
    async fn ticks(&self) -> SseStream {
        SseStream::new(stream::iter([SseEvent::message("tick")]))
    }
}

fn main() {}
