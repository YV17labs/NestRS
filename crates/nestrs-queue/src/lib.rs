//! Redis-backed job queues for nestrs — the NestJS `@nestjs/bullmq` analog,
//! discovered the same way controllers and cron jobs are.
//!
//! A queue has two sides:
//!
//! - **Consumer** — a struct: `#[processor(queue = "welcome-email")]` builds it
//!   from the container (its `#[inject]` fields), implements [`Processor`] for
//!   the logic, and emits the single `impl Discoverable` that attaches a
//!   [`ProcessorMeta`]. The [`QueueWorker`] transport reads those metas from the
//!   fully-assembled container at `configure` and runs one apalis worker per
//!   processor — so a processor is wired by listing it in
//!   `#[module(providers = [...])]`, exactly like a service or controller.
//! - **Producer** — any provider injects [`QueueConnection`] and enqueues:
//!   `self.queue.of::<WelcomeEmail>("welcome-email").push(job).await?`. The queue
//!   name is a string shared with the consuming `#[processor]`.
//!
//! The connection is async, so it is seeded once at the composition root with an
//! [`App::builder`](nestrs_core::App::builder) factory; the `QueueWorker`
//! transport and every producer then resolve it from the container, so import
//! order is irrelevant.
//!
//! ```ignore
//! #[derive(Clone, serde::Serialize, serde::Deserialize)]
//! pub struct WelcomeEmail { pub email: String }
//!
//! #[processor(queue = "welcome-email", concurrency = 5, retries = 3)]
//! pub struct WelcomeEmailWorker {
//!     #[inject] mailer: std::sync::Arc<Mailer>,
//! }
//!
//! #[nestrs_queue::async_trait]
//! impl nestrs_queue::Processor for WelcomeEmailWorker {
//!     type Job = WelcomeEmail;
//!     async fn process(&self, job: WelcomeEmail) -> anyhow::Result<()> {
//!         self.mailer.send_welcome(&job.email).await
//!     }
//! }
//!
//! // main.rs
//! App::builder()
//!     .provide_factory(|_| QueueConnection::connect("redis://127.0.0.1/"))
//!     .module::<AppModule>()
//!     .build().await?
//!     .transport(QueueWorker::new())
//!     .run().await
//! ```

mod config;
mod connection;
mod module;
mod processor;
mod worker;

pub use config::QueueConfig;
pub use connection::{Queue, QueueConnection};
pub use module::{QueueModule, QueueSetup};
pub use processor::{Job, Processor, ProcessorMeta};
pub use worker::QueueWorker;

// `pub` only so `#[processor]`-generated code can name them.
#[doc(hidden)]
pub use processor::{register_worker, FromContainer};

pub use nestrs_queue_macros::processor;

// Re-exported so a `#[processor]` struct can write `#[nestrs_queue::async_trait]`
// on its `Processor` impl without taking a direct `async_trait` dependency.
pub use async_trait::async_trait;
