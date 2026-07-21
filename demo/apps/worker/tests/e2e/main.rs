//! End-to-end suite for the worker app — boots `WorkerModule` against live
//! Redis and asserts an enqueued job is consumed. The module tree mirrors
//! `src/`.

mod module;
