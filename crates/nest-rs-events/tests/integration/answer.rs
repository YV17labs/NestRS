//! Covers `src/answer.rs` from the listener's side: what an `#[on_event]`
//! method answers is read by its type, through a `macro_rules!` fragment as
//! through the type written out.

use nest_rs_core::operation_log::{ERROR, TARGET as OPERATIONS};
use nest_rs_core::{App, injectable, module};
use nest_rs_events::{EventBus, EventsModule, listeners};
use nest_rs_testing::LogCapture;

/// A note went to the archive, which refuses it.
#[derive(Clone)]
struct Archived;

// The answer comes through `tt` fragments, keeping the caller's spans, and
// `#[listeners]` the macro's: what the expansion binds resolves in the answer's.
macro_rules! declare_fragment_listeners {
    ($($answer:tt)*) => {
        #[injectable]
        struct FragmentListeners;

        #[listeners]
        impl FragmentListeners {
            #[on_event]
            async fn on_archived(&self, _event: Archived) -> $($answer)* {
                Err(std::io::Error::other("the archive refused the note"))
            }
        }
    };
}

declare_fragment_listeners!(Result<(), std::io::Error>);

#[module(imports = [EventsModule], providers = [FragmentListeners])]
struct FragmentEventsModule;

#[tokio::test]
async fn an_answer_a_macro_fragment_writes_is_read_as_the_one_written_out() {
    let app = App::builder()
        .module::<FragmentEventsModule>()
        .build()
        .await
        .expect("boots");
    let bus = app.container().get::<EventBus>().expect("the bus");
    let logs = LogCapture::install();

    bus.emit(Archived).await;

    let dispatched = logs.expect_one(OPERATIONS, nest_rs_events::unit::DISPATCH.name());
    assert_eq!(dispatched.field("method").as_deref(), Some("on_archived"));
    assert_eq!(dispatched.field("outcome").as_deref(), Some(ERROR));
}
